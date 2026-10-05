//! Dumps the MATH table of every math font file (face 0) in the index as this crate
//! reads it, for `tools/math-dump.py` (fontTools) to compare:
//!
//!     scripts/sandbox cargo run -p partex-otf --release --example otf-math -- INDEX > mine.txt
//!     scripts/sandbox python3 crates/partex-otf/tools/math-dump.py < mine.txt > theirs.txt

use std::fmt::Write;
use std::sync::Arc;

use partex_otf::index::{FontIndex, LOADABLE};
use partex_otf::math::{KernSide, Math};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let index = FontIndex::from_bytes(&std::fs::read(&args[1]).expect("index")).expect("an index");
    let mut seen = std::collections::BTreeSet::new();
    for e in &index.entries {
        if e.flags & LOADABLE == 0 || e.index != 0 || !seen.insert(e.path.clone()) {
            continue;
        }
        let Ok(data) = std::fs::read(&*e.path) else {
            continue;
        };
        let Some(face) = partex_otf::Face::new(Arc::from(data), e.index) else {
            continue;
        };
        let Some(m) = Math::new(&face) else { continue };
        println!("F {}", e.path);
        let c: Vec<String> = (0..=55).map(|i| m.constant(i).to_string()).collect();
        println!("C {}", c.join(" "));
        println!("O {}", m.min_connector_overlap());
        for g in 0..face.num_glyphs() {
            let ic = m.italics_correction(g);
            let ta = m.top_accent(g);
            let mut line = format!("G {g} {ic} {}", ta.map_or("-".into(), |v| v.to_string()));
            for h in [false, true] {
                let v = m.variants(g, h);
                let (parts, aic) = m.assembly(g, h);
                write!(
                    line,
                    " | {:?} {:?} {aic}",
                    v,
                    parts
                        .iter()
                        .map(|p| (
                            p.glyph,
                            p.start_connector_length,
                            p.end_connector_length,
                            p.full_advance,
                            p.extender
                        ))
                        .collect::<Vec<_>>()
                )
                .unwrap();
            }
            for side in [
                KernSide::TopRight,
                KernSide::TopLeft,
                KernSide::BottomRight,
                KernSide::BottomLeft,
            ] {
                let ks: Vec<i32> = [-1000, -200, 0, 150, 400, 700, 1500]
                    .iter()
                    .map(|&h| m.kerning(g, side, h))
                    .collect();
                write!(line, " {ks:?}").unwrap();
            }
            println!("{line}");
        }
    }
}
