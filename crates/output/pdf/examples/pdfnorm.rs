//! Two PDF files compared after renumbering (`RELAXED.md` gate 2, the
//! comparison `tests/relaxed.rs` makes): `pdfnorm GOT WANT` prints
//! `equal` or the first difference, exit 0 or 1.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [got, want] = &args[..] else {
        eprintln!("usage: pdfnorm GOT WANT");
        std::process::exit(2);
    };
    let read = |p: &str| {
        std::fs::read(p).unwrap_or_else(|e| {
            eprintln!("{p}: {e}");
            std::process::exit(2)
        })
    };
    match phitex_pdf::norm::first_difference(&read(got), &read(want)) {
        None => println!("equal"),
        Some(d) => {
            println!("differs: {d}");
            std::process::exit(1);
        }
    }
}
