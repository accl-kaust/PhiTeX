//! Times this crate's deflate at a level on a file (a measurement):
//! `deflate_time FILE LEVEL [RUNS]` prints the best wall of RUNS and the
//! output's length and Adler-32, to set beside zlib's on the same input;
//! then the resumed compression ([`Resumable`]) on the same input with one
//! byte changed at a few places: each output checked against a whole
//! compression, and timed.

use phitex_codecs::deflate::Resumable;

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
    resumed(&data, level, &out);
}

/// The resumed compression of `data` and of `data` changed at a few
/// places, checked against [`compress`]'s bytes and timed.
fn resumed(data: &[u8], level: i32, whole: &[u8]) {
    let mut memo = Resumable::default();
    let start = std::time::Instant::now();
    let (cold, _) = memo.compress(data, level);
    println!(
        "resumable cold: {:?}, same bytes {}",
        start.elapsed(),
        cold == whole
    );
    let mut changed = data.to_vec();
    for tenth in [9, 7, 5, 3, 1] {
        let at = changed.len() * tenth / 10;
        changed[at] = changed[at].wrapping_add(1);
        let start = std::time::Instant::now();
        let (bytes, again) = memo.compress(&changed, level);
        let wall = start.elapsed();
        let fresh = phitex_codecs::deflate::compress(&changed, level);
        println!(
            "resumed at {at}: {wall:?}, {again} bytes again, same bytes {}",
            bytes == fresh
        );
    }
}
