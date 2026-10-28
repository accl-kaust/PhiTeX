//! `gf_to_pk` against TeX Live's gftopk: each GF file of
//! tests/fixtures, made by TeX Live's mf as mktexpk runs it (made by
//! tests/bless.sh on accl, job 21876: mf and gftopk of TeX Live 2026,
//! versions.txt; the fonts, modes, magnifications, resolutions and md5s
//! in list.txt), packed to the bytes of gftopk's PK file.

use std::path::Path;

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn check(name: &str, dpi: u32) {
    let fx = fixtures();
    let gf = std::fs::read(fx.join(format!("{name}.{dpi}gf"))).expect("the GF file");
    let want = std::fs::read(fx.join(format!("{name}.{dpi}pk"))).expect("gftopk's PK file");
    let got = phitex_gftopk::gf_to_pk(&gf).expect("a PK file");
    if let Some(i) = got.iter().zip(&want).position(|(a, b)| a != b) {
        panic!(
            "{name}.{dpi}pk: byte {i} is {} where gftopk's is {} ({} bytes, gftopk's {})",
            got[i],
            want[i],
            got.len(),
            want.len()
        );
    }
    assert_eq!(got.len(), want.len(), "{name}.{dpi}pk: the length");
}

// (ghab: crbox's corner font; cmr10, cmti10, logo10 at 600 dpi; cminch's
// large characters: the two-byte character preambles; cmr5 at 60 dpi and
// cmtt8 at 120: small characters, bit maps among them)
#[test]
fn ghab_600() {
    check("ghab", 600);
}

#[test]
fn cmr10_600() {
    check("cmr10", 600);
}

#[test]
fn cmti10_600() {
    check("cmti10", 600);
}

#[test]
fn logo10_600() {
    check("logo10", 600);
}

#[test]
fn cminch_600() {
    check("cminch", 600);
}

#[test]
fn cmr5_60() {
    check("cmr5", 60);
}

#[test]
fn cmtt8_120() {
    check("cmtt8", 120);
}

/// What the fixtures exercise: the PK files' character packets by
/// preamble form and packing (run counts or a bit map), so the tests
/// above are known to cover each.
#[test]
fn the_fixtures_cover_every_form() {
    let mut forms = [0usize; 3];
    let (mut runs, mut maps) = (0usize, 0usize);
    for e in std::fs::read_dir(fixtures())
        .expect("the fixtures")
        .flatten()
    {
        let p = e.path();
        if !p
            .extension()
            .is_some_and(|x| x.to_string_lossy().ends_with("pk"))
        {
            continue;
        }
        let pk = std::fs::read(&p).expect("a PK file");
        let mut i = 3 + usize::from(pk[2]) + 16;
        while pk[i] != 245 {
            let flag = pk[i];
            if flag >= 240 {
                i += match flag {
                    240 => 2 + usize::from(pk[i + 1]),
                    244 => 5,
                    _ => 1,
                };
                continue;
            }
            if flag >> 4 == 14 {
                maps += 1;
            } else {
                runs += 1;
            }
            let (form, len) = match flag & 7 {
                0..=3 => (0, usize::from(flag & 3) * 256 + usize::from(pk[i + 1]) + 3),
                4..=6 => (
                    1,
                    usize::from(flag & 3) * 65536
                        + usize::from(pk[i + 1]) * 256
                        + usize::from(pk[i + 2])
                        + 4,
                ),
                _ => (
                    2,
                    u32::from_be_bytes([pk[i + 1], pk[i + 2], pk[i + 3], pk[i + 4]]) as usize + 9,
                ),
            };
            forms[form] += 1;
            i += len;
        }
    }
    eprintln!(
        "short {}, extended short {}, long {}; run counts {runs}, bit maps {maps}",
        forms[0], forms[1], forms[2]
    );
    assert!(
        forms[0] > 0 && forms[1] > 0,
        "short and extended short preambles: {forms:?}"
    );
    assert!(runs > 0 && maps > 0, "run counts {runs}, bit maps {maps}");
}
