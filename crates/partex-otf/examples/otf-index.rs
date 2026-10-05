//! Builds the font index (DESIGN 4.7; format: `partex_otf::index`).
//!
//!     scripts/sandbox cargo run -p partex-otf --release --example otf-index -- OUT [--check-fc]
//!
//! The faces and their order are fontconfig's: `fc-list` run with
//! `scripts/xetex/fonts.conf` (TeX Live's `fonts/opentype` and
//! `fonts/truetype`, exactly what the oracle's XeTeX sees) and XeTeX's
//! own `FcFontList` arguments (`:outline=true` and the objects family,
//! style, file, index, fullname, weight, width, slant, fontformat), whose
//! order XeTeX's name matching depends on. Each face's bytes are read from
//! its absolute path, which the index keeps.
//!
//! `--check-fc` compares fontconfig's family, style and full names with
//! the ones `fontmgr::fc_names` derives from the records.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use partex_otf::index::{FaceEntry, FontIndex, LOADABLE, SFNT, WOFF};
use partex_otf::xetex::fontmgr::fc_names;

struct FcFace {
    file: String,
    index: u32,
    weight: u16,
    width: u16,
    slant: u16,
    family: Vec<String>,
    style: Vec<String>,
    fullname: Vec<String>,
}

fn fc_list(conf: &str) -> Vec<FcFace> {
    let sep = '\u{1f}';
    let fmt = format!(
        "%{{file}}\t%{{index}}\t%{{weight}}\t%{{width}}\t%{{slant}}\t%{{[]family{{%{{family}}{sep}}}}}\t%{{[]style{{%{{style}}{sep}}}}}\t%{{[]fullname{{%{{fullname}}{sep}}}}}\n"
    );
    let out = Command::new("fc-list")
        .env("FONTCONFIG_FILE", conf)
        .arg("-f")
        .arg(fmt)
        .arg(":outline=true")
        .args([
            "family",
            "style",
            "file",
            "index",
            "fullname",
            "weight",
            "width",
            "slant",
            "fontformat",
        ])
        .output()
        .expect("fc-list runs");
    assert!(out.status.success(), "fc-list failed");
    let text = String::from_utf8_lossy(&out.stdout);
    let list = |s: &str| -> Vec<String> {
        s.split(sep)
            .filter(|x| !x.is_empty())
            .map(String::from)
            .collect()
    };
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 8 {
                return None;
            }
            let num = |s: &str| s.parse::<f64>().map_or(0, |v| v as u16);
            Some(FcFace {
                file: f[0].to_string(),
                index: f[1].parse().ok()?,
                weight: num(f[2]),
                width: num(f[3]),
                slant: num(f[4]),
                family: list(f[5]),
                style: list(f[6]),
                fullname: list(f[7]),
            })
        })
        .collect()
}

fn base128(d: &[u8], p: &mut usize) -> Option<u32> {
    let mut v: u32 = 0;
    for _ in 0..5 {
        let b = *d.get(*p)?;
        *p += 1;
        v = (v << 7) | u32::from(b & 0x7f);
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

const WOFF2_TAGS: [&[u8; 4]; 63] = [
    b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"cvt ", b"fpgm",
    b"glyf", b"loca", b"prep", b"CFF ", b"VORG", b"EBDT", b"EBLC", b"gasp", b"hdmx", b"kern",
    b"LTSH", b"PCLT", b"VDMX", b"vhea", b"vmtx", b"BASE", b"GDEF", b"GPOS", b"GSUB", b"EBSC",
    b"JSTF", b"MATH", b"CBDT", b"CBLC", b"COLR", b"CPAL", b"SVG ", b"sbix", b"acnt", b"avar",
    b"bdat", b"bloc", b"bsln", b"cvar", b"fdsc", b"feat", b"fmtx", b"fvar", b"gvar", b"hsty",
    b"just", b"lcar", b"mort", b"morx", b"opbd", b"prop", b"trak", b"Zapf", b"Silf", b"Glat",
    b"Gloc", b"Feat", b"Sill",
];

/// The untransformed tables of a (single-font) WOFF2 file.
fn woff2_tables(d: &[u8]) -> Option<Vec<([u8; 4], Vec<u8>)>> {
    let be32 =
        |o: usize| -> Option<u32> { Some(u32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?)) };
    if be32(4)? == u32::from_be_bytes(*b"ttcf") {
        return None;
    }
    let n = u16::from_be_bytes(d.get(12..14)?.try_into().ok()?) as usize;
    let comp_size = be32(20)? as usize;
    let mut p = 48;
    let mut dir = Vec::with_capacity(n);
    for _ in 0..n {
        let flags = *d.get(p)?;
        p += 1;
        let tag: [u8; 4] = if flags & 0x3f == 0x3f {
            let t = d.get(p..p + 4)?.try_into().ok()?;
            p += 4;
            t
        } else {
            *WOFF2_TAGS[(flags & 0x3f) as usize]
        };
        let version = (flags >> 6) & 3;
        let orig = base128(d, &mut p)?;
        let glyf_loca = &tag == b"glyf" || &tag == b"loca";
        let transformed = if glyf_loca {
            version == 0
        } else {
            version != 0
        };
        let len = if transformed {
            base128(d, &mut p)?
        } else {
            orig
        };
        dir.push((tag, len as usize, transformed));
    }
    let mut out = Vec::new();
    let mut dec = brotli_decompressor::Decompressor::new(d.get(p..p + comp_size)?, 4096);
    dec.read_to_end(&mut out).ok()?;
    let mut q = 0;
    let mut tables = Vec::new();
    for (tag, len, transformed) in dir {
        let b = out.get(q..q + len)?.to_vec();
        q += len;
        if !transformed {
            tables.push((tag, b));
        }
    }
    Some(tables)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out_path = args.get(1).expect("usage: otf-index OUT [--check-fc]");
    let check = args.iter().any(|a| a == "--check-fc");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let conf = root.join("scripts/xetex/fonts.conf");
    let faces = fc_list(conf.to_str().expect("utf-8 path"));
    let mut index = FontIndex::default();
    let (mut mismatches, mut woff2, mut unloadable) = (0, 0, 0);
    for f in &faces {
        let data: Arc<[u8]> = Arc::from(std::fs::read(&f.file).expect("font readable"));
        let mut e = if partex_otf::woff::is_woff2(&data) {
            woff2 += 1;
            match woff2_tables(&data) {
                Some(tables) => {
                    let table = |t: u32| -> Option<&[u8]> {
                        tables
                            .iter()
                            .find(|(tag, _)| u32::from_be_bytes(*tag) == t)
                            .map(|(_, b)| &b[..])
                    };
                    let mut e = FaceEntry::from_tables(&f.file, f.index, true, &table);
                    e.flags |= WOFF;
                    e
                }
                None => FaceEntry::read(&f.file, data.clone(), f.index),
            }
        } else {
            FaceEntry::read(&f.file, data.clone(), f.index)
        };
        // XeTeX reads fontconfig's weight, width and slant where the face
        // gives no OS/2 weight and width.
        // fontconfig's pattern for a variable font itself.
        if f.index == 0 && f.style.is_empty() && f.fullname.is_empty() && !e.names.is_empty() {
            e.flags |= partex_otf::index::VARIABLE;
        }
        let no_os2 = e.os2.is_none_or(|(w, wd, _)| w == 0 && wd == 0);
        if no_os2 {
            e.fc_style = Some((f.weight, f.width, f.slant));
        }
        if e.flags & (LOADABLE | SFNT) != (LOADABLE | SFNT) {
            unloadable += 1;
        }
        if check {
            let mine = fc_names(&e);
            let set = |v: &Vec<String>| {
                v.iter()
                    .filter(|x| !x.is_empty())
                    .cloned()
                    .collect::<BTreeSet<_>>()
            };
            for (what, a, b) in [
                ("family", &mine.family, &f.family),
                ("style", &mine.style, &f.style),
                ("fullname", &mine.fullname, &f.fullname),
            ] {
                if set(a) != set(b) {
                    mismatches += 1;
                    if mismatches <= 40 {
                        eprintln!(
                            "fc {what} {}:{}\n  mine {a:?}\n  fc   {b:?}",
                            f.file, f.index
                        );
                    }
                }
            }
        }
        index.entries.push(e);
    }
    let bytes = index.to_bytes();
    std::fs::write(out_path, &bytes).expect("index written");
    assert_eq!(
        FontIndex::from_bytes(&bytes).as_ref(),
        Some(&index),
        "round trip"
    );
    eprintln!(
        "{} faces ({woff2} WOFF2, {unloadable} not loadable here), {} bytes{}",
        index.entries.len(),
        bytes.len(),
        if check {
            format!(", {mismatches} fontconfig name mismatches")
        } else {
            String::new()
        }
    );
}
