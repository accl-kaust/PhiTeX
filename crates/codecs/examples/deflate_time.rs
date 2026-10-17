//! Times this crate's deflate at a level on a file (a measurement):
//! `deflate_time FILE LEVEL [RUNS]` prints the best wall of RUNS and the
//! output's length and Adler-32, to set beside zlib's on the same input.

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let data = std::fs::read(&a[1]).expect("the input");
    let level: i32 = a[2].parse().expect("a level");
    let runs: usize = a.get(3).and_then(|r| r.parse().ok()).unwrap_or(5);
    let mut best = std::time::Duration::MAX;
    let mut out = Vec::new();
    for _ in 0..runs {
        let t = std::time::Instant::now();
        out = phitex_codecs::deflate::compress(&data, level);
        best = best.min(t.elapsed());
    }
    println!(
        "ours: {} bytes in, {} out, adler {:08x}, best of {runs} {best:?}",
        data.len(),
        out.len(),
        phitex_codecs::deflate::adler32(1, &out)
    );
}
