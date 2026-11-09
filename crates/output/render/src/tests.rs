use std::fmt::Write as _;

use super::*;

/// A one-page PDF (612 by 792 points) whose content is `lines`, each a
/// line of Helvetica text at its height from the page's bottom, then a
/// rule under the last.
fn pdf(lines: &[(f64, &str)]) -> Arc<[u8]> {
    let mut content = String::new();
    for (y, t) in lines {
        let _ = writeln!(content, "BT /F1 12 Tf 72 {y} Td ({t}) Tj ET");
    }
    if let Some((y, _)) = lines.last() {
        let _ = writeln!(content, "q 0 0 1 rg 72 {} 200 1 re f Q", y - 10.0);
    }
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
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
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .as_bytes(),
    );
    Arc::from(out)
}

/// The page composed from its frame's strips on white, RGB.
fn compose(r: &Renderer, f: &Frame) -> Vec<u8> {
    let (w, h) = (f.pw as usize, f.ph as usize);
    let mut out = vec![255u8; w * h * 3];
    for b in &f.bands {
        let s = r.strip(b.key).expect("every band's strip is kept");
        assert_eq!((s.w, s.h), (b.w, b.h), "{b:?}");
        for row in 0..s.h as usize {
            let Ok(y) = usize::try_from(i64::from(b.y) + i64::from(u32::try_from(row).unwrap()))
            else {
                continue;
            };
            if y >= h {
                continue;
            }
            for col in 0..s.w as usize {
                let x = usize::try_from(b.x).unwrap() + col;
                if x >= w {
                    continue;
                }
                let p = &s.rgba[(row * s.w as usize + col) * 4..][..4];
                let a = u32::from(p[3]);
                let o = &mut out[(y * w + x) * 3..][..3];
                for c in 0..3 {
                    o[c] = u8::try_from(
                        (u32::from(p[c]) * a + u32::from(o[c]) * (255 - a) + 127) / 255,
                    )
                    .unwrap();
                }
            }
        }
    }
    out
}

/// The page as hayro draws it whole, on white, RGB.
fn whole(bytes: &Arc<[u8]>, scale: f32) -> Vec<u8> {
    let p = Pdf::new(Arc::new(Bytes(bytes.clone()))).unwrap();
    let pages = p.pages();
    let pix = hayro::render(
        &pages[0],
        &RenderCache::new(),
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &hayro::PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: vello_cpu::color::palette::css::WHITE,
        },
    );
    pix.data_as_u8_slice()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect()
}

const THREE: [(f64, &str); 3] = [
    (700.0, "First line of text"),
    (686.0, "Second line"),
    (672.0, "Third one"),
];

/// Each line its band; the same PDF again draws nothing; the bands
/// composed are the page hayro draws whole, but for the fraction of a
/// pixel each band is moved to a whole pixel.
#[test]
fn lines_are_bands_and_compose_to_the_page() {
    let doc = pdf(&THREE);
    let mut r = Renderer::new();
    let f = r.frame(&doc, 0, 2.0).expect("the page");
    // (three lines and the rule 10 points under the third: four bands)
    assert_eq!(f.bands.len(), 4, "{:?}", f.bands);
    assert_eq!((f.drawn, f.kept), (4, 0));
    assert!(
        f.text.iter().any(|l| l.text.contains("Second line")),
        "{:?}",
        f.text
    );
    let again = r.frame(&doc, 0, 2.0).unwrap();
    assert_eq!((again.drawn, again.kept), (0, 4));
    assert_eq!(again.bands, f.bands);
    let a = compose(&r, &f);
    let b = whole(&doc, 2.0);
    assert_eq!(a.len(), b.len());
    let diff: u64 = a
        .iter()
        .zip(&b)
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    let ink: u64 = b.iter().map(|x| u64::from(255 - x)).sum();
    // (the half pixel a band may move by: little against the ink)
    assert!(
        ink > 0 && diff * 3 < ink,
        "composed differs: {diff} against ink {ink}"
    );
}

/// A window's frame (a zoomed-in viewer's): only the bands it crosses,
/// each cut to it, composed the same pixels as that part of the whole
/// page's frame; its strips within the window, those of the bands it
/// cuts its own, those of the bands inside it the whole page's.
#[test]
#[allow(
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)]
fn a_window_is_that_part_of_the_page() {
    let doc = pdf(&THREE);
    let mut r = Renderer::new();
    let f = r.frame(&doc, 0, 2.0).unwrap();
    let full = compose(&r, &f);
    // (from the middle of the first line's band to below the rule, from
    // left of the lines to the middle of the second and the rule)
    let win = [0.1, 0.12, 0.2, 0.4];
    let g = r.frame_in(&doc, 0, 2.0, Some(win)).unwrap();
    let [wx, wy, ww, wh] = g.win;
    assert_eq!((g.pw, g.ph), (f.pw, f.ph));
    assert!(ww < f.pw && wh < f.ph && wx > 0 && wy > 0, "{:?}", g.win);
    assert!(!g.bands.is_empty() && g.bands.len() <= f.bands.len());
    // (a band the window cuts is a strip of its own; one inside it is the
    // whole page's)
    let cut = g.bands.iter().filter(|b| !f.bands.contains(b)).count();
    assert!(cut > 0, "{:?} of {:?}", g.bands, f.bands);
    assert_eq!(g.drawn, cut, "a cut band's strip is its own");
    let (ww, wh) = (ww as usize, wh as usize);
    let mut part = vec![255u8; ww * wh * 3];
    for b in &g.bands {
        let st = r.strip(b.key).expect("kept");
        assert_eq!((st.w, st.h), (b.w, b.h));
        assert!(
            b.y >= wy as i32
                && b.y + st.h as i32 <= (wy as usize + wh) as i32
                && b.x >= wx as i32
                && b.x + st.w as i32 <= (wx as usize + ww) as i32,
            "{b:?} in {:?}",
            g.win
        );
        let sw = st.w as usize;
        for row in 0..st.h as usize {
            let y = (b.y - wy as i32) as usize + row;
            for col in 0..sw {
                let x = (b.x - wx as i32) as usize + col;
                let p = &st.rgba[(row * sw + col) * 4..][..4];
                let a = u32::from(p[3]);
                let o = &mut part[(y * ww + x) * 3..][..3];
                for c in 0..3 {
                    o[c] = u8::try_from(
                        (u32::from(p[c]) * a + u32::from(o[c]) * (255 - a) + 127) / 255,
                    )
                    .unwrap();
                }
            }
        }
    }
    let pw = f.pw as usize;
    let mut differ = 0;
    for y in 0..wh {
        let from = ((wy as usize + y) * pw + wx as usize) * 3;
        if part[y * ww * 3..][..ww * 3] != full[from..][..ww * 3] {
            differ += 1;
        }
    }
    assert_eq!(differ, 0, "rows of the window unlike the page's");
    // (the same window again: drawn not again)
    let again = r.frame_in(&doc, 0, 2.0, Some(win)).unwrap();
    assert_eq!((again.drawn, again.kept), (0, g.bands.len()));
}

/// A line added above: the lines below are the same strips at new
/// places, drawn not again; a changed line is drawn again alone.
#[test]
fn moved_lines_are_kept_and_a_changed_line_is_drawn() {
    let mut r = Renderer::new();
    let f0 = r.frame(&pdf(&THREE), 0, 1.5).unwrap();
    // (the three lines and the rule pushed down a line, 14 points; at
    // 1.5 pixels a point, 21 pixels)
    let moved = [
        (700.0, "A new line above"),
        (686.0, "First line of text"),
        (672.0, "Second line"),
        (658.0, "Third one"),
    ];
    let f1 = r.frame(&pdf(&moved), 0, 1.5).unwrap();
    assert_eq!(
        (f1.drawn, f1.kept),
        (1, 4),
        "{:?} then {:?}",
        f0.bands,
        f1.bands
    );
    let changed = [
        (700.0, "First line of text"),
        (686.0, "Second lime"),
        (672.0, "Third one"),
    ];
    let f2 = r.frame(&pdf(&changed), 0, 1.5).unwrap();
    assert_eq!((f2.drawn, f2.kept), (1, 3));
    assert_eq!(f2.bands[0].key, f0.bands[0].key);
    assert_ne!(f2.bands[1].key, f0.bands[1].key);
}

/// A link's box and target, a URI's and a page's.
#[test]
fn links_are_read() {
    let content = "BT /F1 12 Tf 72 700 Td (Link) Tj ET\n";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R /Annots [6 0 R 7 0 R] >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        "<< /Type /Annot /Subtype /Link /Rect [72 698 100 712] /A << /S /URI /URI (https://example.org/) >> >>".to_owned(),
        "<< /Type /Annot /Subtype /Link /Rect [72 600 100 612] /Dest [3 0 R /XYZ 0 500 null] >>".to_owned(),
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
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .as_bytes(),
    );
    let doc: Arc<[u8]> = Arc::from(out);
    let f = Renderer::new().frame(&doc, 0, 1.0).unwrap();
    assert_eq!(f.links.len(), 2, "{:?}", f.links);
    assert!(matches!(&f.links[0].target, Target::Uri(u) if u == "https://example.org/"));
    assert!(
        (f.links[0].rect[1] - 80.0).abs() < 1e-6,
        "{:?}",
        f.links[0].rect
    );
    assert!(
        matches!(f.links[1].target, Target::Page(0, Some(y)) if (y - 292.0).abs() < 1e-6),
        "{:?}",
        f.links[1]
    );
}

/// The outline's entries, nested, with their pages and places; a page's
/// text by lines.
#[test]
fn outline_and_text_are_read() {
    let content =
        "BT /F1 12 Tf 72 700 Td (Hello outline) Tj ET\nBT /F1 12 Tf 72 680 Td (Second) Tj ET\n";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        "<< /Type /Outlines /First 7 0 R /Last 8 0 R /Count 3 >>".to_owned(),
        "<< /Title (Intro) /Parent 6 0 R /Next 8 0 R /First 9 0 R /Last 9 0 R /Dest [3 0 R /XYZ 0 700 null] >>".to_owned(),
        "<< /Title <FEFF00C9007400E9> /Parent 6 0 R /Prev 7 0 R /A << /S /GoTo /D [3 0 R /FitH 500] >> >>".to_owned(),
        "<< /Title (Inside) /Parent 7 0 R /Dest [3 0 R /XYZ 0 600 null] >>".to_owned(),
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
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .as_bytes(),
    );
    let doc: Arc<[u8]> = Arc::from(out);
    let mut r = Renderer::new();
    let o = r.outline(&doc);
    assert_eq!(o.len(), 2, "{o:?}");
    assert_eq!(
        (o[0].title.as_str(), o[0].page, o[0].y),
        ("Intro", Some(0), Some(92.0))
    );
    assert_eq!(o[0].kids.len(), 1);
    assert_eq!(
        (o[0].kids[0].title.as_str(), o[0].kids[0].y),
        ("Inside", Some(192.0))
    );
    assert_eq!((o[1].title.as_str(), o[1].y), ("Été", Some(292.0)));
    assert_eq!(r.pages(&doc), Some(1));
    let t = r.text(&doc, 0).expect("the page's text");
    let lines: Vec<&str> = t.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(lines, ["Hello outline", "Second"], "{t:?}");
    assert!(
        (t[0].y - 92.0).abs() < 0.01 && (t[0].size - 12.0).abs() < 0.01,
        "{:?}",
        t[0]
    );
    assert_eq!(t[0].xs.len(), "Hello outline".encode_utf16().count());
}

/// `objs` as a PDF's objects 1, 2, ... (1 the catalog), with its xref.
fn pdf_of(objs: &[String]) -> Arc<[u8]> {
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
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .as_bytes(),
    );
    Arc::from(out)
}

/// A one-page PDF of `content` with Helvetica as /F1 and a Type 3 font
/// as /F2 (glyphs a and b, 500 units wide, drawn 20 to 480).
fn pdf_with_type3(content: &str) -> Arc<[u8]> {
    let proc_a = "500 0 20 0 480 600 d1 20 0 460 600 re f";
    let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /T3 def 1 begincodespacerange <00> <FF> endcodespacerange 2 beginbfchar <61> <0061> <62> <0062> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> /Contents 4 0 R >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        "<< /Type /Font /Subtype /Type3 /FontMatrix [0.001 0 0 0.001 0 0] /FontBBox [0 0 500 600] /CharProcs << /a 7 0 R /b 7 0 R >> /Encoding << /Type /Encoding /Differences [97 /a /b] >> /FirstChar 97 /LastChar 98 /Widths [500 500] /ToUnicode 8 0 R /Resources << >> >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{proc_a}\nendstream", proc_a.len() + 1),
        format!("<< /Length {} >>\nstream\n{cmap}\nendstream", cmap.len() + 1),
    ];
    pdf_of(&objs)
}

/// Links and outline entries to named destinations: the catalog's
/// `/Dests`, and a `/Names` tree of two leaves under a root, its values
/// an object, an array inside the leaf, a dictionary's `/D`; a name in
/// neither is no link. Each name's place found once a document (the
/// tree read once), as a search of the tree for every link found it.
#[test]
fn named_destinations_are_found() {
    let content = "BT /F1 12 Tf 72 700 Td (Links) Tj ET\n";
    let link = |y: u32, dest: &str| {
        format!(
            "<< /Type /Annot /Subtype /Link /Rect [72 {y} 100 {}] {dest} >>",
            y + 10
        )
    };
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /Names << /Dests 9 0 R >> /Dests << /old [3 0 R /XYZ 0 100 null] >> /Outlines 14 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R /Annots [6 0 R 7 0 R 8 0 R 15 0 R 16 0 R] >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        link(700, "/Dest (a)"),
        link(680, "/A << /S /GoTo /D (b) >>"),
        link(660, "/Dest /c"),
        "<< /Kids [10 0 R 11 0 R] >>".to_owned(),
        "<< /Limits [(a) (b)] /Names [(a) 12 0 R (b) [3 0 R /XYZ 0 400 null]] >>".to_owned(),
        "<< /Limits [(c) (d)] /Names [(c) 13 0 R] >>".to_owned(),
        "[3 0 R /XYZ 0 500 null]".to_owned(),
        "<< /D [3 0 R /FitH 300] >>".to_owned(),
        "<< /Type /Outlines /First 17 0 R /Last 17 0 R /Count 1 >>".to_owned(),
        link(640, "/Dest (old)"),
        link(620, "/Dest (zz)"),
        "<< /Title (To b) /Parent 14 0 R /Dest (b) >>".to_owned(),
    ];
    let doc = pdf_of(&objs);
    let mut r = Renderer::new();
    let f = r.frame(&doc, 0, 1.0).unwrap();
    let tops: Vec<(f64, usize, Option<f64>)> = f
        .links
        .iter()
        .map(|l| match l.target {
            Target::Page(p, y) => (l.rect[1], p, y),
            Target::Uri(_) => panic!("{l:?}"),
        })
        .collect();
    // (pages' tops: 792 less the destination's y)
    assert_eq!(
        tops,
        [
            (82.0, 0, Some(292.0)),
            (102.0, 0, Some(392.0)),
            (122.0, 0, Some(492.0)),
            (142.0, 0, Some(692.0)),
        ],
        "{:?}",
        f.links
    );
    let o = r.outline(&doc);
    assert_eq!(
        (o[0].title.as_str(), o[0].page, o[0].y),
        ("To b", Some(0), Some(392.0))
    );
}

/// Two columns: each line of the left column a band of its own beside a
/// figure (a filled box) in the right column that spans them all, not one
/// band of the lines and the figure: a changed line is drawn again alone,
/// and the bands compose to the page.
#[test]
fn a_column_beside_a_figure_is_lines() {
    let doc = |second: &str| {
        let mut content = String::new();
        for (y, t) in [
            (700, "First line of text"),
            (686, second),
            (672, "Third one"),
        ] {
            let _ = writeln!(content, "BT /F1 12 Tf 72 {y} Td ({t}) Tj ET");
        }
        content.push_str("q 0 0 1 rg 320 600 200 120 re f Q\n");
        pdf_of(&[
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_owned(),
            format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        ])
    };
    let mut r = Renderer::new();
    let a = doc("Second line");
    let f = r.frame(&a, 0, 2.0).unwrap();
    // (three lines and the figure)
    assert_eq!(f.bands.len(), 4, "{:?}", f.bands);
    let fig = f.bands.iter().filter(|b| b.x > 500).count();
    assert_eq!(fig, 1, "{:?}", f.bands);
    let composed = compose(&r, &f);
    let page = whole(&a, 2.0);
    let diff: u64 = composed
        .iter()
        .zip(&page)
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    let ink: u64 = page.iter().map(|x| u64::from(255 - x)).sum();
    assert!(
        ink > 0 && diff * 3 < ink,
        "composed differs: {diff} against ink {ink}"
    );
    let g = r.frame(&doc("Second lime"), 0, 2.0).unwrap();
    assert_eq!((g.drawn, g.kept), (1, 3), "{:?}", g.bands);
    let px: u32 = g
        .bands
        .iter()
        .filter(|b| !f.bands.contains(b))
        .map(|b| b.w * b.h)
        .sum();
    assert!(
        px * 10 < f.pw * f.ph,
        "{px} pixels drawn again of {}",
        f.pw * f.ph
    );
}

/// The text layer's word spaces: a gap of .15em between words (a
/// justified line's shrunk space: Grail's Libertine lines ran their words
/// together at the fifth of the em the layer took) is a space, a kern of
/// .05em is not; and a Type 3 font's letters (pdfTeX's bitmap fonts)
/// are no words apart, their words are.
#[test]
fn word_spaces_are_read_from_the_gaps() {
    let content = "BT /F1 12 Tf 72 700 Td [(alpha) -150 (beta) 50 (gamma)] TJ ET\n\
                   BT /F2 12 Tf 72 650 Td [(ab) -300 (ab)] TJ ET\n\
                   BT /F2 12 Tf 72 600 Td [(ab) -150 (ba)] TJ ET\n";
    let doc = pdf_with_type3(content);
    let mut r = Renderer::new();
    let lines: Vec<String> = r
        .text(&doc, 0)
        .expect("the page's text")
        .into_iter()
        .map(|l| l.text)
        .collect();
    assert_eq!(lines, ["alpha betagamma", "ab ab", "ab ba"]);
}

/// `data` as a zlib stream of stored (uncompressed) deflate blocks: a
/// `/FlateDecode` stream without a compressor.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut chunks = data.chunks(0xffff).peekable();
    if chunks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(c) = chunks.next() {
        out.push(u8::from(chunks.peek().is_none()));
        let n = u16::try_from(c.len()).unwrap();
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&(!n).to_le_bytes());
        out.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    out.extend_from_slice(&((b << 16) | a).to_be_bytes());
    out
}

/// Rows of `stride` bytes PNG-predicted (`/Predictor` 10 to 15), each row
/// its filter byte first: None, Sub and Up in turn, `bpp` bytes a pixel
/// (at least one), as a PNG's IDAT that pdfTeX copies holds them.
fn png_predicted(raw: &[u8], stride: usize, bpp: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut above = vec![0u8; stride];
    for (y, row) in raw.chunks(stride).enumerate() {
        let kind = u8::try_from(y % 3).unwrap();
        out.push(kind);
        for (i, &x) in row.iter().enumerate() {
            out.push(match kind {
                0 => x,
                1 => x.wrapping_sub(if i >= bpp { row[i - bpp] } else { 0 }),
                _ => x.wrapping_sub(above[i]),
            });
        }
        above.copy_from_slice(row);
    }
    out
}

/// Objects (1 the catalog) as a PDF, each its bytes: streams binary.
fn pdf_of_bytes(objs: &[Vec<u8>]) -> Arc<[u8]> {
    let mut out = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut at = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        at.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
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
    Arc::from(out)
}

/// A stream object: `dict`'s entries, its `/Length`, its bytes.
fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut o = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    o.extend_from_slice(data);
    o.extend_from_slice(b"\nendstream");
    o
}

/// An image's place on the page (left, bottom, points) and its
/// quadrants' colours.
type Placed = ((f64, f64), [[u8; 3]; 4]);

/// `b` in hex, two digits a byte.
fn hex(b: &[u8]) -> String {
    b.iter().fold(String::new(), |mut s, v| {
        let _ = write!(s, "{v:02x}");
        s
    })
}

/// The four colours of the test images' quadrants: top left, top right,
/// bottom left, bottom right (`testdata/quads.jpg` holds them too).
const QUADS: [[u8; 3]; 4] = [[200, 30, 30], [30, 160, 60], [40, 60, 200], [230, 200, 40]];

/// Image samples 32 by 32, each quadrant its value of `q`.
fn quads<const N: usize>(q: [[u8; N]; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    for y in 0..32 {
        for x in 0..32 {
            out.extend_from_slice(&q[usize::from(y >= 16) * 2 + usize::from(x >= 16)]);
        }
    }
    out
}

/// A page of every kind of image pdfTeX and XeTeX write, each 64 points
/// square at its place (left, bottom) on the page, and each one's
/// quadrants' colours composed on white: an RGB PNG's samples as pdfTeX
/// copies them (PNG predictors), a JPEG (`/DCTDecode`), a PNG with alpha
/// (an `/SMask`), a palette PNG (`/Indexed`, 4 bits a sample), a grey
/// one with a `/Decode` array and `/Interpolate`, an `/ICCBased` RGB one,
/// an image mask (`/ImageMask`, filled with the fill colour), an inline
/// image, and a Type 3 font's bitmap glyph (pdfTeX's PK fonts: an inline
/// image mask in the glyph's procedure).
fn images_pdf() -> (Arc<[u8]>, Vec<Placed>) {
    let rgb = quads(QUADS);
    let half =
        |c: [u8; 3]| c.map(|v| u8::try_from((u32::from(v) * 128 + 255 * 127 + 127) / 255).unwrap());
    let alpha = quads([[255], [255], [128], [128]]);
    // (4-bit palette indices 0 to 3 by quadrant: two samples a byte)
    let indexed: Vec<u8> = quads([[0], [1], [2], [3]])
        .chunks(2)
        .map(|p| (p[0] << 4) | p[1])
        .collect();
    let palette = hex(QUADS.as_flattened());
    let grey = quads([[0], [64], [128], [255]]);
    // (mask bits: 0 paints; the top left and bottom right painted)
    let mask: Vec<u8> = (0..32)
        .flat_map(|y| {
            let top = y < 16;
            if top {
                [0x00, 0x00, 0xff, 0xff]
            } else {
                [0xff, 0xff, 0x00, 0x00]
            }
        })
        .collect();
    let white = [255, 255, 255];
    let red = QUADS[0];
    let places: Vec<Placed> = vec![
        ((72.0, 600.0), QUADS),
        ((172.0, 600.0), QUADS),
        (
            (272.0, 600.0),
            [QUADS[0], QUADS[1], half(QUADS[2]), half(QUADS[3])],
        ),
        ((372.0, 600.0), QUADS),
        ((72.0, 450.0), [white, [191; 3], [127; 3], [0; 3]]),
        ((172.0, 450.0), QUADS),
        ((272.0, 450.0), [red, white, white, red]),
        ((372.0, 450.0), [red, white, white, red]),
        ((72.0, 300.0), [red; 4]),
    ];
    let mut content = String::from("BT /F1 12 Tf 72 720 Td (Images) Tj ET\n");
    for (i, ((x, y), _)) in places.iter().take(6).enumerate() {
        let _ = writeln!(content, "q 64 0 0 64 {x} {y} cm /I{i} Do Q");
    }
    let _ = writeln!(
        content,
        "q 0.784 0.118 0.118 rg 64 0 0 64 272 450 cm /I6 Do Q"
    );
    let _ = writeln!(
        content,
        "q 0.784 0.118 0.118 rg 64 0 0 64 372 450 cm BI /W 32 /H 32 /IM true /BPC 1 /F /AHx ID {}> EI Q",
        hex(&mask)
    );
    // (the glyph is 1000 units square: 64 points at a size of 64)
    content.push_str("BT 0.784 0.118 0.118 rg /F2 64 Tf 72 300 Td (a) Tj ET\n");
    let glyph = format!(
        "1000 0 0 0 1000 1000 d1 q 1000 0 0 1000 0 0 cm BI /W 8 /H 8 /IM true /BPC 1 /F /AHx ID {}> EI Q",
        hex(&[0; 8])
    );
    let common = "/Type /XObject /Subtype /Image /Width 32 /Height 32";
    let objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R /F2 6 0 R >> /XObject << /I0 8 0 R /I1 9 0 R /I2 10 0 R /I3 11 0 R /I4 12 0 R /I5 13 0 R /I6 14 0 R >> >> /Contents 4 0 R >>".to_vec(),
        stream("", content.as_bytes()),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        b"<< /Type /Font /Subtype /Type3 /FontMatrix [0.001 0 0 0.001 0 0] /FontBBox [0 0 1000 1000] /CharProcs << /a 7 0 R >> /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] /Resources << >> >>".to_vec(),
        stream("", glyph.as_bytes()),
        stream(
            &format!("{common} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /DecodeParms << /Predictor 15 /Colors 3 /BitsPerComponent 8 /Columns 32 >>"),
            &zlib_stored(&png_predicted(&rgb, 96, 3)),
        ),
        stream(
            &format!("{common} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode"),
            include_bytes!("testdata/quads.jpg"),
        ),
        stream(
            &format!("{common} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /SMask 15 0 R"),
            &zlib_stored(&rgb),
        ),
        stream(
            &format!("{common} /ColorSpace [/Indexed /DeviceRGB 3 <{palette}>] /BitsPerComponent 4 /Filter /FlateDecode /DecodeParms << /Predictor 15 /Colors 1 /BitsPerComponent 4 /Columns 32 >>"),
            &zlib_stored(&png_predicted(&indexed, 16, 1)),
        ),
        stream(
            &format!("{common} /ColorSpace /DeviceGray /BitsPerComponent 8 /Decode [1 0] /Interpolate true /Filter /FlateDecode"),
            &zlib_stored(&grey),
        ),
        stream(
            &format!("{common} /ColorSpace [/ICCBased 16 0 R] /BitsPerComponent 8 /Filter /FlateDecode"),
            &zlib_stored(&rgb),
        ),
        stream(
            &format!("{common} /ImageMask true /BitsPerComponent 1"),
            &mask,
        ),
        stream(
            &format!("{common} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode"),
            &zlib_stored(&alpha),
        ),
        stream("/N 3 /Alternate /DeviceRGB", b""),
    ];
    (pdf_of_bytes(&objs), places)
}

/// Every kind of image drawn in the strips, as poppler draws it: each
/// image's quadrants their colours (a JPEG's within its loss), the bands
/// composed the page hayro draws whole, and, where poppler's `pdftoppm`
/// is installed, the page it draws. (An image's box was its transform's
/// unit square, its corner sample: no band held a raster image, and every
/// PNG and JPEG figure was blank in the viewer.)
#[test]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a page's pixels at 72 dpi: points are pixels"
)]
fn every_kind_of_image_is_drawn() {
    let (doc, places) = images_pdf();
    let mut r = Renderer::new();
    let f = r.frame(&doc, 0, 1.0).expect("the page");
    assert_eq!((f.pw, f.ph), (612, 792));
    let page = compose(&r, &f);
    let at = |p: &[u8], x: f64, y: f64| -> [u8; 3] {
        let i = ((792.0 - y) as usize * 612 + x as usize) * 3;
        [p[i], p[i + 1], p[i + 2]]
    };
    let mut bad = Vec::new();
    for (k, ((x, y), want)) in places.iter().enumerate() {
        // (the JPEG's loss: a few levels a channel)
        let tol = if k == 1 { 16 } else { 4 };
        for (q, w) in want.iter().enumerate() {
            let (dx, dy) = (
                if q % 2 == 0 { 16.0 } else { 48.0 },
                if q < 2 { 48.0 } else { 16.0 },
            );
            let got = at(&page, x + dx, y + dy);
            if got.iter().zip(w).any(|(g, w)| g.abs_diff(*w) > tol) {
                bad.push(format!("image {k} quadrant {q}: {got:?}, not {w:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}\nbands {:?}", f.bands);
    let whole = whole(&doc, 1.0);
    let diff: u64 = page
        .iter()
        .zip(&whole)
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    let ink: u64 = whole.iter().map(|x| u64::from(255 - x)).sum();
    assert!(
        ink > 0 && diff * 50 < ink,
        "composed differs: {diff} against ink {ink}"
    );
    let dir = std::env::temp_dir().join(format!("phitex-render-images-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("images.pdf"), &doc[..]).unwrap();
    let run = std::process::Command::new("pdftoppm")
        .args(["-r", "72", "-singlefile"])
        .arg(dir.join("images.pdf"))
        .arg(dir.join("poppler"))
        .status();
    if !run.is_ok_and(|s| s.success()) {
        eprintln!("pdftoppm not run: the quadrants checked alone");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    let ppm = std::fs::read(dir.join("poppler.ppm")).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let head = b"P6\n612 792\n255\n";
    assert!(ppm.starts_with(head), "{:?}", &ppm[..16]);
    let pop = &ppm[head.len()..];
    for (k, ((x, y), _)) in places.iter().enumerate() {
        // (each image's inside but its edges and its quadrants' edges:
        // poppler smooths an image it draws larger, two pixels a sample
        // here, where hayro takes the nearest sample; elsewhere the same
        // pixels, a JPEG's decoders a few levels apart)
        let tol = if k == 1 { 32 } else { 4 };
        let top = (792.0 - y - 64.0) as usize;
        let left = *x as usize;
        let mut worst = (0, 0, 0);
        for py in top + 2..top + 62 {
            for px in left + 2..left + 62 {
                if py.abs_diff(top + 32) < 3 || px.abs_diff(left + 32) < 3 {
                    continue;
                }
                let i = (py * 612 + px) * 3;
                let d = (0..3)
                    .map(|c| page[i + c].abs_diff(pop[i + c]))
                    .max()
                    .unwrap();
                worst = worst.max((d, px, py));
            }
        }
        assert!(
            worst.0 <= tol,
            "image {k} differs from poppler's by {} at {:?}",
            worst.0,
            (worst.1, worst.2)
        );
    }
}

/// Identical lines are one band key: drawn once a frame, and counted once
/// against the strips' budget. Each drawn and kept, the count grew by
/// every repeat at every zoom, past [`KEEP_BYTES`] for good, and the
/// watch then dropped every strip as soon as it drew it: the viewer's
/// pages came blank (16 of a page's 17 strips, after some zooming).
#[test]
fn repeated_lines_are_counted_once_and_kept() {
    let lines: Vec<(f64, &str)> = (0..12)
        .map(|i| (700.0 - 40.0 * f64::from(i), "The same line"))
        .collect();
    let doc = pdf(&lines);
    let mut r = Renderer::new();
    for scale in [1.0, 1.5, 2.0, 1.5, 3.0, 1.0] {
        let f = r.frame(&doc, 0, scale).expect("the page");
        let keys: std::collections::HashSet<u64> = f.bands.iter().map(|b| b.key).collect();
        assert!(
            keys.len() < f.bands.len(),
            "the lines share a key: {:?}",
            f.bands
        );
        assert!(
            f.drawn <= keys.len(),
            "a key drawn once: {} drawn of {} keys",
            f.drawn,
            keys.len()
        );
        for b in &f.bands {
            assert!(r.strip(b.key).is_some(), "band {b:?} kept at {scale}");
        }
    }
    let held: usize = r.strips.values().map(|(s, _)| s.rgba.len()).sum();
    assert_eq!(r.bytes, held, "the budget counts each strip once");
}

/// A frame's bands are the newest strips: drawing past the budget drops
/// the oldest, never a band a frame just listed (kept from before), which
/// the viewer fetches next.
#[test]
fn a_frames_kept_bands_outlive_the_ones_drawn_before_it() {
    let a = pdf(&[(700.0, "Kept line"), (600.0, "Another")]);
    let big = pdf(&[(700.0, "A wide line drawn at many scales")]);
    let mut r = Renderer::new();
    let listed = r.frame(&a, 0, 1.0).expect("the page");
    // (other strips drawn after them, to three quarters of the budget)
    let mut scale = 20.0;
    let mut older = Vec::new();
    while r.bytes < KEEP_BYTES / 4 * 3 {
        older.extend(r.frame(&big, 0, scale).unwrap().bands.iter().map(|b| b.key));
        scale += 1.0;
    }
    // (the page asked for again: its bands kept, listed)
    assert_eq!(r.frame(&a, 0, 1.0).unwrap().drawn, 0);
    // (then drawn past the budget, until the first of those is dropped)
    while older.iter().all(|k| r.strip(*k).is_some()) {
        assert!(scale < 400.0, "the budget never passed");
        r.frame(&big, 0, scale).unwrap();
        scale += 1.0;
    }
    assert!(
        older.iter().any(|k| r.strip(*k).is_none()),
        "the budget passed: the oldest dropped"
    );
    for b in &listed.bands {
        assert!(
            r.strip(b.key).is_some(),
            "a band the frame listed dropped before older ones: {b:?}"
        );
    }
}

/// A page naming resources its file lacks (a page streamed before the
/// objects it draws with are written: its resources' references name no
/// object, or a free one) is drawn all the same, never refused: a missing
/// graphics state leaves the drawing opaque, a missing shading and a
/// fill with a missing pattern draw nothing (a flat fill would cover
/// what lies under a shaded background), a missing colour space falls
/// back to gray, a missing external object draws nothing.
#[test]
fn missing_resources_never_fail_a_frame() {
    let content = "/G1 gs 1 0 0 rg 10 10 20 20 re f \
                   q 40 10 20 20 re W n /S1 sh Q \
                   /Pattern cs /P1 scn 70 10 20 20 re f \
                   /C1 cs 0.5 scn 100 10 20 20 re f \
                   q 20 0 0 20 130 10 cm /X1 Do Q \
                   /Nope gs /Nope cs 0 0 1 scn 160 10 20 20 re f";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        // (6 a free entry, 40 past the cross-reference section)
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /ExtGState 40 0 R /Shading 6 0 R /Pattern 40 0 R /ColorSpace << /C1 41 0 R >> /XObject << /X1 6 0 R >> >> /Contents 4 0 R >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len() + 1),
        "<< >>".to_owned(),
    ];
    let mut bytes = pdf_of(&objs).to_vec();
    // (object 6 named in the cross-reference section as free)
    let s = String::from_utf8_lossy(&bytes).into_owned();
    let s = s.replace("xref\n0 6\n", "xref\n0 7\n").replacen(
        "trailer\n<< /Size 6",
        "0000000000 00001 f \ntrailer\n<< /Size 7",
        1,
    );
    bytes = s.into_bytes();
    let pdf: Arc<[u8]> = Arc::from(bytes);
    let mut r = Renderer::new();
    let f = r.frame(&pdf, 0, 1.0).expect("the page drawn");
    assert_ne!(f.bands.len(), 0, "no band drawn");
    let px = compose(&r, &f);
    let at = |x: usize, y: usize| {
        let i = (y * f.pw as usize + x) * 3;
        [px[i], px[i + 1], px[i + 2]]
    };
    // (points to pixels: the page's top left)
    assert_eq!(at(20, 180), [255, 0, 0], "opaque red");
    assert_eq!(at(50, 180), [255, 255, 255], "no shading");
    assert_eq!(at(80, 180), [255, 255, 255], "no pattern");
    let g = at(110, 180);
    assert!(
        g[0] == g[1] && g[1] == g[2] && (100..160).contains(&g[0]),
        "gray: {g:?}"
    );
    assert_eq!(at(140, 180), [255, 255, 255], "no external object");
    // (and the whole page as hayro draws it, the same)
    assert_eq!(px, whole(&pdf, 1.0));
}
