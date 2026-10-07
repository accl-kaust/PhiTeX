//! Whether two PDFs render alike: `same A.pdf B.pdf`. The pages' draw
//! lists (glyphs, positions, paths, images, links and their targets) and
//! the outline must be equal; object numbers, their order, the xref and
//! resource names may differ (a parallel build's output contract,
//! DESIGN.md "Parallel builds"). Exit 0 when alike, 1 when not.

fn open(file: &str) -> (std::sync::Arc<[u8]>, phitex_draw::Pdf) {
    let data: std::sync::Arc<[u8]> = std::fs::read(file).expect("readable").into();
    let pdf = phitex_draw::Pdf::open(&data).expect("a PDF");
    (data, pdf)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (a, b) = (
        args.next().expect("usage: same A.pdf B.pdf"),
        args.next().expect("usage: same A.pdf B.pdf"),
    );
    let ((_, pa), (_, pb)) = (open(&a), open(&b));
    let (na, nb) = (pa.page_count(), pb.page_count());
    if na != nb {
        println!("pages: {na} and {nb}");
        std::process::exit(1);
    }
    let (mut fa, mut fb) = (phitex_draw::Fonts::new(), phitex_draw::Fonts::new());
    for k in 0..na {
        let (da, db) = (pa.draw(k, &mut fa), pb.draw(k, &mut fb));
        if da != db {
            println!("page {} draws otherwise", k + 1);
            std::process::exit(1);
        }
    }
    if pa.outline() != pb.outline() {
        println!("the outlines differ");
        std::process::exit(1);
    }
    println!("alike: {na} pages");
}
