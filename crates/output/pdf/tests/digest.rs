//! `PdfState::digest`: equal for equal states made by different steps,
//! changed by any single mutation.

use phitex_pdf::{ObjBody, PdfBytes, PdfParams, PdfState};

type Mutation = Box<dyn Fn(&mut PdfState)>;

fn fresh() -> PdfState {
    PdfState::new(PdfParams {
        date: "D:20260101000000Z".to_owned(),
        ..PdfParams::default()
    })
}

#[test]
fn equal_content_equal_digest() {
    let mut a = fresh();
    a.info(b"/A 1 /B 2");
    a.obj_reserve();
    a.obj_reserve();
    let mut b = fresh();
    let mut bytes = PdfBytes::new();
    b.info(b"/A 1 ");
    b.flush(&mut bytes);
    b.obj_reserve();
    b.info(b"/B 2");
    b.obj_reserve();
    let mut ab = PdfBytes::new();
    a.flush(&mut ab);
    b.flush(&mut bytes);
    assert_eq!(a.digest(), b.digest());
    assert_eq!(fresh().digest(), fresh().digest());
}

#[test]
fn any_mutation_changes_digest() {
    let base = fresh().digest();
    let muts: Vec<(&str, Mutation)> = vec![
        ("info", Box::new(|s| s.info(b"/X 1"))),
        ("names", Box::new(|s| s.names(b"/X 1"))),
        ("trailer", Box::new(|s| s.trailer(b"/X 1"))),
        ("trailerid", Box::new(|s| s.trailer_id(b"[<00><00>]"))),
        (
            "catalog",
            Box::new(|s| {
                let _ = s.catalog(b"/X 1", None);
            }),
        ),
        (
            "newobjnum",
            Box::new(|s| {
                s.new_objnum();
            }),
        ),
        (
            "reserve",
            Box::new(|s| {
                s.obj_reserve();
            }),
        ),
        (
            "obj",
            Box::new(|s| {
                s.obj(None, false, None, ObjBody::Text(b"<<>>".to_vec()));
            }),
        ),
        (
            "pageref",
            Box::new(|s| {
                s.page_ref(3);
            }),
        ),
        (
            "mapline",
            Box::new(|s| s.map_line(b"=cmr10 CMR10 <cmr10.pfb")),
        ),
        ("g2u", Box::new(|s| s.glyph_to_unicode(b"ff", b"0066 0066"))),
        (
            "colorstack",
            Box::new(|s| {
                let _ = s.colorstack_init(false, false, b"0 g");
            }),
        ),
        ("param", Box::new(|s| s.params_mut().compress_level = 3)),
    ];
    let mut seen = vec![base];
    for (name, m) in &muts {
        let mut s = fresh();
        m(&mut s);
        let d = s.digest();
        assert_ne!(d, base, "{name} left the digest unchanged");
        assert!(
            !seen.contains(&d) || *name == "newobjnum",
            "{name}: collision"
        );
        seen.push(d);
    }
    // an object's text: same shape, other content
    let mut a = fresh();
    a.obj(None, false, None, ObjBody::Text(b"<<>>".to_vec()));
    let mut b = fresh();
    b.obj(None, false, None, ObjBody::Text(b"[ ]".to_vec()));
    assert_ne!(a.digest(), b.digest());
}
