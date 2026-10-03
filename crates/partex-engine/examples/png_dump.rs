//! What `partex_engine::png` reads of PNG files, printed as the libpng
//! harness (`scripts/png-check/harness.c`) prints what libpng reads, for
//! comparing the two:
//!
//!     png_dump [-x] FILE...
//!
//! For each file the info `read_info` returns, then for each set of
//! transformations pdfTeX can ask for (tRNS to alpha iff tRNS is valid;
//! strip alpha or not with an alpha channel; strip 16 or not at 16 bits)
//! the transformed format and an FNV-1a hash of the rows (`-x`: the rows
//! in hex), or the error.

use std::fmt::Write as _;

use partex_engine::png::{self, Info, Transforms};

fn main() {
    let mut hex = false;
    for path in std::env::args().skip(1) {
        if path == "-x" {
            hex = true;
            continue;
        }
        println!("== {path}");
        let Ok(data) = std::fs::read(&path) else {
            println!("cannot open");
            continue;
        };
        match png::read_info(&data) {
            Ok(info) => {
                print_info(&info);
                print_images(&data, &info, hex);
            }
            Err(e) => println!("info error: {e}"),
        }
    }
}

/// The info line, the palette and the tRNS chunk.
fn print_info(info: &Info) {
    let mut line = format!(
        "info: {}x{} depth {} color {} interlace {} valid {:x} trns-valid {}",
        info.width,
        info.height,
        info.bit_depth,
        info.color_type,
        info.interlace,
        info.valid,
        u8::from(info.get_valid(png::INFO_TRNS) != 0),
    );
    if info.valid & png::INFO_GAMA != 0 {
        write!(line, " gamma {}", info.gamma).unwrap();
    }
    if let Some((x, y, unit)) = info.phys {
        write!(line, " phys {x},{y},{unit}").unwrap();
    }
    println!("{line}");
    if info.valid & png::INFO_PLTE != 0 {
        let mut line = format!("palette: {} ", info.palette.len());
        for c in &info.palette {
            write!(line, "{:02x}{:02x}{:02x}", c[0], c[1], c[2]).unwrap();
        }
        println!("{line}");
    }
    if info.valid & png::INFO_TRNS != 0 {
        let mut line = format!("trns: {} ", info.num_trans);
        if info.color_type == png::COLOR_PALETTE {
            for a in &info.trans_alpha {
                write!(line, "{a:02x}").unwrap();
            }
        } else {
            let [g, r, gr, b] = info.trans_color;
            write!(line, "{g},{r},{gr},{b}").unwrap();
        }
        println!("{line}");
    }
}

/// The image read with each set of transformations pdfTeX can ask for.
fn print_images(data: &[u8], info: &Info, hex: bool) {
    let trns = info.get_valid(png::INFO_TRNS) != 0;
    let alpha = info.color_type & 4 != 0;
    let b16 = info.bit_depth == 16;
    for strip_alpha in [false, true] {
        for strip_16 in [false, true] {
            if (strip_alpha && !alpha) || (strip_16 && !b16) {
                continue;
            }
            let t = Transforms {
                trns_to_alpha: trns,
                strip_alpha,
                strip_16,
            };
            let name = format!(
                "{}{}{}",
                u8::from(trns),
                u8::from(strip_alpha),
                u8::from(strip_16)
            );
            match png::read_image(data, info, t) {
                Err(e) => println!("image {name}: error: {e}"),
                Ok(img) => {
                    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
                    for &b in &img.rows {
                        hash ^= u64::from(b);
                        hash = hash.wrapping_mul(0x100_0000_01b3);
                    }
                    if hex {
                        for r in img.rows.chunks(img.rowbytes.max(1)) {
                            let mut line = String::new();
                            for b in r {
                                write!(line, "{b:02x}").unwrap();
                            }
                            println!("{line}");
                        }
                    }
                    println!(
                        "image {name}: depth {} color {} rowbytes {} interlace {} fnv {hash:016x}",
                        img.bit_depth, img.color_type, img.rowbytes, info.interlace
                    );
                }
            }
        }
    }
}
