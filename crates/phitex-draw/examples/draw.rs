//! A PDF page's draw list (v2) on stdout: `draw FILE.pdf [PAGE]` (from 1).

fn main() {
    let mut args = std::env::args().skip(1);
    let file = args.next().expect("usage: draw FILE.pdf [PAGE]");
    let page: usize = args.next().map_or(1, |p| p.parse().expect("a page number"));
    let data: std::sync::Arc<[u8]> = std::fs::read(&file).expect("readable").into();
    let pdf = phitex_draw::Pdf::open(&data).expect("a PDF");
    let d = pdf
        .draw(page - 1, &mut phitex_draw::Fonts::new())
        .expect("a page");
    println!("{d}");
}
