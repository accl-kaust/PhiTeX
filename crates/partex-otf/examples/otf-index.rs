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

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::doc_markdown,
    clippy::many_single_char_names,
    clippy::too_many_lines,
    clippy::match_same_arms,
    clippy::unreadable_literal
)]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use partex_otf::index::{FaceEntry, FontIndex, LOADABLE, SFNT};
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
        if partex_otf::woff::is_woff2(&data) {
            woff2 += 1;
        }
        let mut e = FaceEntry::read(&f.file, data.clone(), f.index);
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
