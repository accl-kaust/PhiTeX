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
