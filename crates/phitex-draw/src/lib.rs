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

mod glyphs;
mod image;
pub mod pdfdraw;
mod shade;
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
        // (an image changed under the same name changes the page's hash)
        let mut b = objs.clone();
        b[4] = b[4].replace("\u{ff}\u{0}\u{0}\u{0}", "\u{0}\u{0}\u{0}\u{0}");
        assert_ne!(
            crate::pdfdraw::hashes(&a),
            crate::pdfdraw::hashes(&raw_pdf(&b))
        );
    }

    /// The PNG of data URI `uri` (as `image.rs` writes them: 8-bit RGB
    /// or RGBA, each row filter 0): width, height and RGBA pixels.
    fn png_rgba(uri: &str) -> (usize, usize, Vec<u8>) {
        let b64 = uri.strip_prefix("data:image/png;base64,").unwrap();
        let mut bytes = Vec::new();
        let (mut acc, mut bits) = (0u32, 0);
        for c in b64.bytes().filter(|&c| c != b'=') {
            let v = match c {
                b'A'..=b'Z' => c - b'A',
                b'a'..=b'z' => c - b'a' + 26,
                b'0'..=b'9' => c - b'0' + 52,
                b'+' => 62,
                _ => 63,
            };
            acc = (acc << 6) | u32::from(v);
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                bytes.push(u8::try_from((acc >> bits) & 0xff).unwrap());
            }
        }
        let be = |i: usize| u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
        let (w, h, ctype) = (be(16), be(20), bytes[25]);
        let (mut at, mut z) = (8, Vec::new());
        while at + 8 <= bytes.len() {
            let len = be(at);
            if &bytes[at + 4..at + 8] == b"IDAT" {
                z.extend_from_slice(&bytes[at + 8..at + 8 + len]);
            }
            at += 12 + len;
        }
        let rows = miniz_oxide::inflate::decompress_to_vec_zlib(&z).unwrap();
        let n = if ctype == 6 { 4 } else { 3 };
        let mut px = Vec::new();
        for row in rows.chunks(1 + w * n) {
            assert_eq!(row[0], 0);
            for p in row[1..].chunks(n) {
                px.extend_from_slice(&[p[0], p[1], p[2], if n == 4 { p[3] } else { 255 }]);
            }
        }
        (w, h, px)
    }

    /// The data URIs in a draw list's `"I"`.
    fn image_uris(d: &str) -> Vec<&str> {
        d.match_indices("data:")
            .map(|(i, _)| &d[i..i + d[i..].find('"').unwrap()])
            .collect()
    }

    /// A graphics state's constant alphas (`TikZ`'s `opacity`): the colours
    /// drawn as `#rrggbbaa`, back to opaque at `Q`; a blend mode is counted
    /// as not drawn, as is an operator not known.
    #[test]
    fn alphas() {
        let content = "q /A gs 1 0 0 rg 0 0 1 RG 0 0 10 10 re B Q 0 0 10 10 re f \
                       q /B gs 0 0 10 10 re f Q 1 0 0 1 0 0 zz";
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R \
             /Resources << /ExtGState << /A << /ca 0.5 /CA 0.25 >> /B << /BM /Multiply >> >> >> >>"
                .to_owned(),
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
        ];
        let d = crate::Pdf::open(&raw_pdf(&objs))
            .unwrap()
            .draw(0, &mut crate::Fonts::new())
            .unwrap();
        assert!(d.contains("\"#ff000080\",\"#0000ff40\""), "{d}");
        assert!(
            d.contains("[\"M0 100L10 100L10 90L0 90Z\",\"#000000\",null"),
            "{d}"
        );
        assert!(d.contains("\"x\":2"), "{d}");
    }

    /// `/ImageMask true` on an image `XObject` (a boolean in the file's
    /// dictionary, not a content stream's operator): a stencil, painted
    /// in the fill colour where its bit is 0.
    #[test]
    fn image_masks() {
        let content = "1 0 0 rg q 8 0 0 1 0 0 cm /Im1 Do Q";
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
            "<< /Type /XObject /Subtype /Image /Width 8 /Height 1 /ImageMask true \
             /Length 1 >>\nstream\n\u{f}\nendstream"
                .to_owned(),
        ];
        let d = crate::Pdf::open(&raw_pdf(&objs))
            .unwrap()
            .draw(0, &mut crate::Fonts::new())
            .unwrap();
        assert!(!d.contains("\"x\":"), "{d}");
        let uris = image_uris(&d);
        assert_eq!(uris.len(), 1, "{d}");
        let (w, h, px) = png_rgba(uris[0]);
        assert_eq!((w, h), (8, 1));
        let red = [255, 0, 0, 255];
        for (x, p) in px.chunks(4).enumerate() {
            assert_eq!(p[3], if x < 4 { 255 } else { 0 }, "{x}");
            if x < 4 {
                assert_eq!(p, red);
            }
        }
    }

    /// Shadings, as images over the box of the clip they paint: an
    /// axial one by `sh` inside a clip; a radial one (a sampled function,
    /// gray) as a pattern's fill colour, through the pattern's matrix,
    /// clipped to the path it fills.
    #[test]
    fn shadings() {
        let content = "q 10 10 100 50 re W n /Sh1 sh Q /Pattern cs /P1 scn 150 10 40 40 re f";
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R \
             /Resources << /Shading << /Sh1 5 0 R >> /Pattern << /P1 6 0 R >> >> >>"
                .to_owned(),
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
            "<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [10 0 110 0] /Extend [true true] \
             /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> >>"
                .to_owned(),
            "<< /PatternType 2 /Matrix [1 0 0 1 170 30] /Shading << /ShadingType 3 \
             /ColorSpace /DeviceGray /Coords [0 0 0 0 0 20] /Extend [false true] \
             /Function 7 0 R >> >>"
                .to_owned(),
            "<< /FunctionType 0 /Domain [0 1] /Range [0 1] /Size [2] /BitsPerSample 8 \
             /Length 2 >>\nstream\n\u{0}\u{ff}\nendstream"
                .to_owned(),
        ];
        let d = crate::Pdf::open(&raw_pdf(&objs))
            .unwrap()
            .draw(0, &mut crate::Fonts::new())
            .unwrap();
        assert!(!d.contains("\"x\":"), "{d}");
        let (head, tail) = d.split_once(",\"I\":{").unwrap();
        let ids: Vec<&str> = head
            .match_indices("[\"i")
            .map(|(i, _)| &head[i + 2..i + 19])
            .collect();
        assert_eq!(
            head,
            format!(
                "{{\"v\":2,\"w\":200,\"h\":100,\"f\":[],\"F\":[],\"g\":{{}},\"t\":[],\"p\":[],\
                 \"r\":[[\"{}\",100,0,0,50,10,40,\"c0\"],[\"{}\",40,0,0,40,150,50,\"c1\"]]",
                ids[0], ids[1]
            )
        );
        assert!(
            tail.ends_with(
                "\"C\":{\"c0\":[\"M10 90L110 90L110 40L10 40Z\",0],\
                 \"c1\":[\"M150 90L190 90L190 50L150 50Z\",0]}}"
            ),
            "{tail}"
        );
        let px = |uri: &str, x: usize, y: usize| {
            let (w, _, px) = png_rgba(uri);
            px[(y * w + x) * 4..(y * w + x) * 4 + 4].to_vec()
        };
        let uris = image_uris(&d);
        let (axial, radial) = if tail.find(ids[0]) < tail.find(ids[1]) {
            (uris[0], uris[1])
        } else {
            (uris[1], uris[0])
        };
        // (1.5 pixels a point: 150 × 75; red to blue from left to right)
        assert_eq!(png_rgba(axial).0, 150);
        assert_eq!(px(axial, 0, 0), [254, 0, 1, 255]);
        assert_eq!(px(axial, 75, 30), [127, 0, 128, 255]);
        assert_eq!(px(axial, 149, 74), [1, 0, 254, 255]);
        // (black at the centre, white from the circle out: extended)
        let (w, h, _) = png_rgba(radial);
        assert_eq!((w, h), (60, 60));
        assert!(px(radial, 30, 30)[0] < 10);
        // (10.33 points from the centre of a circle of 20)
        assert_eq!(px(radial, 45, 30)[..2], [132, 132]);
        assert_eq!(px(radial, 0, 0), [255, 255, 255, 255]);
    }

    /// Glyphs from outlines of fonts other than Type 1 programs: a Type 3
    /// font's glyph procedure (its glyph space a hundredth of an em: the
    /// path and the advance scaled to the others'; a stroke left out), a
    /// `TrueType` program (`/FontFile2`, `DejaVu` Sans's A) and a bare CFF
    /// one (`/FontFile3 /Type1C`, Latin Modern Roman's A).
    #[test]
    fn glyph_outlines() {
        let bytes = |b: &[u8]| b.iter().map(|&c| char::from(c)).collect::<String>();
        let ttf = bytes(include_bytes!("../tests/data/dejavu-A.ttf"));
        let cff = bytes(include_bytes!("../tests/data/lmroman-A.cff"));
        let stream =
            |d: &str, s: &str| format!("<< {d} /Length {} >>\nstream\n{s}\nendstream", s.len());
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R /F2 7 0 R /F3 9 0 R >> >> >>"
                .to_owned(),
            stream(
                "",
                "BT /F1 10 Tf 10 50 Td (AA) Tj /F2 10 Tf (A) Tj /F3 10 Tf (A) Tj ET",
            ),
            "<< /Type /Font /Subtype /Type3 /FontMatrix [0.01 0 0 0.01 0 0] \
             /FontBBox [0 0 50 50] /CharProcs << /sq 6 0 R >> \
             /Encoding << /Differences [65 /sq] >> /FirstChar 65 /Widths [50] >>"
                .to_owned(),
            stream(
                "",
                "50 0 0 0 50 50 d1 0 0 m 50 0 l 50 50 l 0 50 l h f 10 10 m 20 20 l S",
            ),
            "<< /Type /Font /Subtype /TrueType /BaseFont /DejaVuSans /FirstChar 65 \
             /Widths [684] /FontDescriptor 8 0 R >>"
                .to_owned(),
            "<< /Type /FontDescriptor /Flags 32 /FontFile2 11 0 R >>".to_owned(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /LMRoman10-Regular /FirstChar 65 \
             /Widths [750] /FontDescriptor 10 0 R >>"
                .to_owned(),
            "<< /Type /FontDescriptor /Flags 32 /FontFile3 12 0 R >>".to_owned(),
            stream("", &ttf),
            stream("/Subtype /Type1C", &cff),
        ];
        let d = crate::Pdf::open(&raw_pdf(&objs))
            .unwrap()
            .draw(0, &mut crate::Fonts::new())
            .unwrap();
        assert!(d.contains("\"0:65\":\"M0 0L500 0L500 500L0 500Z\""), "{d}");
        assert!(d.contains("[-1,10,50,\"10 15\",\"\",0,[65,65]]"), "{d}");
        assert!(d.contains("[-1,10,50,\"20\",\"\",1,[65]]"), "{d}");
        assert!(d.contains("[-1,10,50,\"26.84\",\"\",2,[65]]"), "{d}");
        assert!(
            d.contains(
                "\"1:65\":\"M342 632L208 269L476 269ZM286 729L398 729L676 0L573 0L507 187L178 187\
                 L112 0L8 0Z\""
            ),
            "{d}"
        );
        assert!(
            d.contains("\"2:65\":\"M717 0L717 31L699 31C639 31 625 38 614 71L398 696"),
            "{d}"
        );
    }

    #[test]
    fn forms() {
        // (a form through its matrix, with its own fonts; a form inside
        // it with no resources, its parent's; a form that paints itself,
        // stopped 16 deep and counted as not drawn)
        let stream =
            |d: &str, s: &str| format!("<< {d} /Length {} >>\nstream\n{s}\nendstream", s.len());
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R \
             /Resources << /XObject << /Fm1 5 0 R >> >> >>"
                .to_owned(),
            stream("", "q 2 0 0 2 10 10 cm /Fm1 Do Q"),
            stream(
                "/Type /XObject /Subtype /Form /Matrix [1 0 0 1 5 0] /Resources \
                 << /Font << /F2 6 0 R >> /XObject << /Fm2 7 0 R /Fm3 8 0 R >> >>",
                "0 0 1 rg 0 0 m 10 0 l 10 10 l f BT /F2 5 Tf (A) Tj ET /Fm2 Do /Fm3 Do",
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /CMR10 /FirstChar 65 /Widths [750] >>"
                .to_owned(),
            stream("/Type /XObject /Subtype /Form", "0 g 0 0 m 1 0 l S"),
            stream(
                "/Type /XObject /Subtype /Form /Resources << /XObject << /Fm3 8 0 R >> >>",
                "/Fm3 Do",
            ),
        ];
        let d = crate::Pdf::open(&raw_pdf(&objs))
            .unwrap()
            .draw(0, &mut crate::Fonts::new())
            .unwrap();
        assert_eq!(
            d,
            "{\"v\":2,\"w\":200,\"h\":100,\"f\":[\"roman\"],\"F\":[\"CMR10\"],\"g\":{},\
             \"t\":[[0,10,90,\"20\",\"A\",0,\"#0000ff\"]],\
             \"p\":[[\"M20 90L40 90L40 70\",\"#0000ff\",null,2],[\"M20 90L22 90\",null,\"#000000\",2]],\
             \"r\":[],\"o\":[[0,0,1],[2,0,1],[0,1,1]],\"x\":1}"
        );
    }

    #[test]
    fn clips() {
        // (a clip, one inside it (even-odd), restored by Q; a form's /BBox)
        let stream =
            |d: &str, s: &str| format!("<< {d} /Length {} >>\nstream\n{s}\nendstream", s.len());
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> /XObject << /Fm1 6 0 R >> >> >>"
                .to_owned(),
            stream(
                "",
                "q 0 0 50 50 re W n q 10 10 m 20 10 l 20 20 l h W* n \
                 0 g 0 0 m 100 0 l S BT /F1 10 Tf (A) Tj ET Q 0 0 m 1 1 l S Q \
                 /Fm1 Do 5 5 m 6 6 l S",
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /CMR10 /FirstChar 65 /Widths [750] >>"
                .to_owned(),
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Matrix [1 0 0 1 100 0]",
                "0 0 m 20 20 l S",
            ),
        ];
        let d = crate::Pdf::open(&raw_pdf(&objs))
            .unwrap()
            .draw(0, &mut crate::Fonts::new())
            .unwrap();
        assert_eq!(
            d,
            "{\"v\":2,\"w\":200,\"h\":100,\"f\":[\"roman\"],\"F\":[\"CMR10\"],\"g\":{},\
             \"t\":[[0,10,100,\"0\",\"A\",0,null,\"c1\"]],\
             \"p\":[[\"M0 100L100 100\",null,\"#000000\",1,\"c1\"],\
             [\"M0 100L1 99\",null,\"#000000\",1,\"c0\"],\
             [\"M100 100L120 80\",null,\"#000000\",1,\"c2\"],\
             [\"M5 95L6 94\",null,\"#000000\",1]],\"r\":[],\
             \"C\":{\"c0\":[\"M0 100L50 100L50 50L0 50Z\",0],\
             \"c1\":[\"M10 90L20 90L20 80Z\",1,\"c0\"],\
             \"c2\":[\"M100 100L110 100L110 90L100 90Z\",0]},\
             \"o\":[[0,0,1],[2,0,1],[0,1,3]]}"
        );
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
