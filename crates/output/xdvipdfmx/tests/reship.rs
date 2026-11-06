//! A page written again ([`Session::reship`]) into a file whose objects
//! are written: its forms (`pdf:bxobj @name ... pdf:exobj`, TikZ's
//! transparency groups and shadings) made again under the labels the
//! file has them by, so that every page drawing a form, written again or
//! not, draws the new one, and the form's object is the one a file made
//! from the first page on writes.

use std::sync::Arc;

use phitex_xdvipdfmx::api::{Options, Session};
use phitex_xdvipdfmx::direct::Op;
use phitex_xdvipdfmx::io::{Files, Format};

/// No files: the pages draw no glyphs.
struct NoFiles;

impl Files for NoFiles {
    fn find(&self, _: &[u8], _: Format, _: &[u8]) -> Option<Vec<u8>> {
        None
    }
    fn read(&self, _: &[u8]) -> Option<Arc<[u8]>> {
        None
    }
}

fn session() -> Session {
    let options = Options {
        pdf_filename: Some(b"forms.pdf".to_vec()),
        source_date_epoch: Some(0),
        // (streams as they are: their bytes found in the file)
        args: vec![b"-z".to_vec(), b"0".to_vec()],
        ..Options::default()
    };
    Session::direct(
        options,
        Arc::new(NoFiles),
        Box::new(|_, d: &[u8]| d.to_vec()),
        1000,
        b" XeTeX output",
    )
    .unwrap()
}

fn sp(s: &str) -> Op {
    Op::Special(s.as_bytes().to_vec())
}

/// Page 1 makes form `@shade` (drawn there and on page 2) and the
/// transparency group `@group` (drawn there), the form's stroke colour
/// `colour`.
fn pages(colour: &str) -> Vec<Vec<Op>> {
    vec![
        vec![
            sp("pdf:bxobj @shade width 20pt height 10pt"),
            sp(&format!("pdf:code {colour} RG 0 0 m 20 10 l S")),
            sp("pdf:exobj"),
            sp("pdf:uxobj @shade"),
            Op::Down(655_360),
            sp("pdf:bxobj @group bbox 0 0 30 30"),
            sp(&format!("pdf:code {colour} rg 0 0 30 30 re f")),
            sp("pdf:exobj <</Group << /S /Transparency /I false /K false >> >>"),
            sp("pdf:uxobj @group"),
        ],
        vec![sp("pdf:uxobj @shade"), sp("pdf:code 0 0 m 5 5 l S")],
    ]
}

/// Each object `N 0 obj ... endobj` of `pdf` whose body has `what`: its
/// number and body.
fn objects_with(pdf: &[u8], what: &[u8]) -> Vec<(u32, Vec<u8>)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(o) = find(&pdf[at..], b" 0 obj") {
        let start = at + o;
        let n0 = pdf[..start]
            .iter()
            .rposition(|c| !c.is_ascii_digit())
            .map_or(0, |p| p + 1);
        let n: u32 = std::str::from_utf8(&pdf[n0..start])
            .unwrap()
            .parse()
            .unwrap();
        let body_at = start + b" 0 obj".len();
        let end = body_at + find(&pdf[body_at..], b"endobj").unwrap();
        let body = pdf[body_at..end].to_vec();
        if find(&body, what).is_some() {
            out.push((n, body));
        }
        at = end;
    }
    out
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

#[test]
fn a_page_written_again_makes_its_forms_again() {
    // (the file of the first colour: its form objects)
    let mut s = session();
    let mut base = Vec::new();
    for p in pages("1 0 0") {
        base.extend(s.page_ops(&p).unwrap().pdf);
    }
    let post = s.snapshot();
    base.extend(s.finish().unwrap());
    let size = s.next_label();
    eprintln!("{}", String::from_utf8_lossy(&base));
    let forms = objects_with(&base, b"/Subtype /Form");
    assert_eq!(forms.len(), 2, "the file's forms");

    // (page 1 written again in the second colour, into that file)
    let mut again = post.snapshot();
    let now = pages("0 0 1");
    let written = again.reship(0, &now[0], size).unwrap().pdf;
    let made = objects_with(&written, b"/Subtype /Form");
    let numbers = |x: &[(u32, Vec<u8>)]| x.iter().map(|f| f.0).collect::<Vec<_>>();
    assert_eq!(
        numbers(&made),
        numbers(&forms),
        "the forms written again are not the file's objects"
    );

    // (each one the object a file of the second colour from its first
    // page on has, by its body)
    let mut exact = session();
    let mut whole = Vec::new();
    for p in &now {
        whole.extend(exact.page_ops(p).unwrap().pdf);
    }
    whole.extend(exact.finish().unwrap());
    let want = objects_with(&whole, b"/Subtype /Form");
    let bodies = |x: &[(u32, Vec<u8>)]| x.iter().map(|f| f.1.clone()).collect::<Vec<_>>();
    assert_eq!(bodies(&made), bodies(&want), "the forms' bodies");
    assert!(
        made.iter().all(|f| find(&f.1, b"0 0 1").is_some()),
        "the forms written again are not the new ones"
    );

    // (the page written again draws them under the names the file's
    // page did)
    let page = |pdf: &[u8]| {
        objects_with(pdf, b" Do")
            .into_iter()
            .map(|x| x.1)
            .filter(|b| find(b, b"/Subtype /Form").is_none())
            .collect::<Vec<_>>()
    };
    let (a, b) = (page(&written), page(&base));
    assert!(!a.is_empty() && a[0] == b[0], "the page's content stream");
}

/// The names of the forms specials begin, as a page written again makes
/// them again.
#[test]
fn form_names() {
    let mut s = session();
    let name = |s: &mut Session, x: &str| s.form_defined(x.as_bytes()).unwrap();
    assert_eq!(
        name(&mut s, "pdf:bxobj @pgftransgroup3 bbox 0 0 1 1"),
        Some(b"pgftransgroup3".to_vec())
    );
    assert_eq!(
        name(&mut s, "pdf:beginxobj @a width 1pt height 1pt"),
        Some(b"a".to_vec())
    );
    assert_eq!(name(&mut s, "pdf:uxobj @a"), None);
    assert_eq!(name(&mut s, "pdf:exobj"), None);
    assert_eq!(name(&mut s, "pdf:code 0 0 m"), None);
}
