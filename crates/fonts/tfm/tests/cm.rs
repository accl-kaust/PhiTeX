//! The Computer Modern TFM files TeX Live installs, against `tftopl` and
//! TeX itself.
//!
//! Checksums and design sizes are from
//! `tftopl -charcode-format=octal tests/fixtures/<font>.tfm` (TeX Live
//! 2026). The widths in scaled points are TeX's own: `tex -ini` on
//! `\font\y=<font> at <size>pt \setbox0\hbox{\y A}\message{\number\wd0}`
//! (the PL's `CHARWD R` values are the same widths rounded to six
//! decimals: cmr10 'A' 0.750002, 'f' 0.305557, 'i' 0.277779).

#![allow(clippy::unreadable_literal)]

use phitex_tfm::{Tfm, TfmError, parse};

fn load(name: &str) -> Tfm {
    let path = format!("{}/tests/fixtures/{name}.tfm", env!("CARGO_MANIFEST_DIR"));
    parse(&std::fs::read(path).unwrap()).unwrap()
}

const PT: i32 = 65536;

/// (font, `CHECKSUM O`, widths of A, f, i at 10pt, the same at 12pt)
const FONTS: [(&str, u32, [i32; 3], [i32; 3]); 5] = [
    (
        "cmr10",
        0o11374260171,
        [491521, 200250, 182045],
        [589825, 240300, 218454],
    ),
    (
        "cmbx10",
        0o3274421126,
        [569796, 230285, 209350],
        [683755, 276342, 251220],
    ),
    (
        "cmti10",
        0o37500023472,
        [487148, 200976, 200976],
        [584577, 241171, 241171],
    ),
    (
        "cmsy10",
        0o4110426232,
        [523286, 327681, 254863],
        [627943, 393217, 305835],
    ),
    (
        "cmex10",
        0o37254272422,
        [573441, 655361, 309476],
        [688130, 786434, 371372],
    ),
];

#[test]
fn checksums_sizes_widths() {
    for (name, checksum, w10, w12) in FONTS {
        let tfm = load(name);
        assert_eq!(u32::from_be_bytes(tfm.checksum), checksum, "{name}");
        // (DESIGNSIZE R 10.0)
        assert_eq!(tfm.design_size, 10 * PT, "{name}");
        let at10 = tfm.at(tfm.size_for(-1000)).unwrap();
        let at12 = tfm.at(12 * PT).unwrap();
        for (k, c) in b"Afi".iter().copied().enumerate() {
            assert_eq!(at10.width(c), Some(w10[k]), "{name} {}", c as char);
            assert_eq!(at12.width(c), Some(w12[k]), "{name} {}", c as char);
        }
    }
}

#[test]
fn header_strings() {
    let tfm = load("cmr10");
    assert_eq!(tfm.coding_scheme.as_deref(), Some(&b"TeX text"[..]));
    assert_eq!(tfm.family.as_deref(), Some(&b"CMR"[..]));
    assert_eq!(tfm.param_count(), 7);
    assert_eq!(load("cmsy10").param_count(), 22);
    assert_eq!(load("cmex10").param_count(), 13);
}

#[test]
fn ffi_ligatures() {
    // tftopl: (LABEL O 146) (LIG O 151 O 14) (LIG O 146 O 13) ...
    let tfm = load("cmr10");
    let ff = tfm.lookup(Some(b'f'), Some(b'f')).unwrap();
    assert!(!ff.is_kern());
    assert_eq!((ff.op, ff.remainder), (0, 11));
    let ffi = tfm.lookup(Some(11), Some(b'i')).unwrap();
    assert_eq!((ffi.op, ffi.remainder), (0, 14));
    let fi = tfm.lookup(Some(b'f'), Some(b'i')).unwrap();
    assert_eq!(fi.remainder, 12);
    assert_eq!(tfm.lookup(Some(b'f'), Some(b'x')), None);
    // cmr10 has no boundary character
    assert_eq!(tfm.bchar(), None);
    assert_eq!(tfm.lookup(Some(b'f'), None), None);
    assert_eq!(tfm.lookup(None, Some(b'f')), None);
}

#[test]
fn av_kern() {
    // tftopl: (LABEL O 101) ... (KRN O 126 R -0.111112); TeX:
    // \hbox{AV} is 910223sp, \hbox{A\kern0pt V} 983042sp.
    let tfm = load("cmr10");
    let k = tfm.lookup(Some(b'A'), Some(b'V')).unwrap();
    assert!(k.is_kern());
    let at10 = tfm.at(10 * PT).unwrap();
    assert_eq!(at10.char_kern(k), 910223 - 983042);
}

#[test]
fn params() {
    let at10 = load("cmr10").at(10 * PT).unwrap();
    // tftopl: (SLANT R 0.0) (SPACE R 0.333334) (QUAD R 1.000003); TeX:
    // \number\fontdimen6 is 655361, \fontdimen2 218453, cmti10 \fontdimen1 16384
    assert_eq!(at10.slant(), 0);
    assert_eq!(at10.quad(), 655361);
    assert_eq!(at10.space(), 218453);
    let cmti = load("cmti10").at(10 * PT).unwrap();
    // (SLANT R 0.25)
    assert_eq!(cmti.slant(), 0o200000 / 4);
}

#[test]
fn truncated() {
    let path = format!("{}/tests/fixtures/cmr10.tfm", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(path).unwrap();
    let err = parse(&bytes[..bytes.len() / 2]).unwrap_err();
    assert_eq!(err, TfmError::Truncated);
    assert!(err.to_string().starts_with("Bad metric (TFM) file"));
    // one byte short: the last parameter is incomplete (§575)
    assert_eq!(
        parse(&bytes[..bytes.len() - 1]).unwrap_err(),
        TfmError::Truncated
    );
}
