//! The port's side of the unit check (tools/harness/gr2dump.c prints the
//! same for graphite2): each task's segment, its slots with their floats'
//! bits.
//!
//!     gr-dump TASKS > OUT
//!
//! A task line: FONT, LANG, FEATS, DIR, TEXT, tab separated (see
//! gr2dump.c).

#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;

use phitex_graphite::Face;

fn tables(data: &[u8]) -> HashMap<u32, Vec<u8>> {
    let mut out = HashMap::new();
    if data.len() < 12 {
        return out;
    }
    let be = |p: usize| u32::from_be_bytes([data[p], data[p + 1], data[p + 2], data[p + 3]]);
    let n = usize::from(u16::from_be_bytes([data[4], data[5]]));
    for i in 0..n {
        let e = 12 + 16 * i;
        if e + 16 > data.len() {
            break;
        }
        let (tag, off, len) = (be(e), be(e + 8) as usize, be(e + 12) as usize);
        if off + len <= data.len() {
            out.entry(tag)
                .or_insert_with(|| data[off..off + len].to_vec());
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let tasks = std::fs::read_to_string(&args[1]).expect("tasks");
    let mut faces: HashMap<String, Option<Face>> = HashMap::new();
    let mut out = String::new();
    let mut task = 0u64;
    for line in tasks.lines() {
        let f: Vec<&str> = line.splitn(5, '\t').collect();
        if f.len() < 5 {
            continue;
        }
        task += 1;
        let face = faces.entry(f[0].to_string()).or_insert_with(|| {
            let data = std::fs::read(f[0]).ok()?;
            let t = tables(&data);
            Face::new(&|tag| t.get(&tag).cloned())
        });
        let Some(face) = face else {
            writeln!(out, "T{task} noface").unwrap();
            continue;
        };
        let lang = if f[1] == "-" {
            0
        } else {
            let mut t = [0u8; 4];
            for (i, b) in f[1].bytes().take(4).enumerate() {
                t[i] = b;
            }
            u32::from_be_bytes(t)
        };
        let mut feats = face.featureval_for_lang(lang);
        if f[2] != "-" {
            for kv in f[2].split(',') {
                let (k, v) = kv.split_once('=').unwrap_or((kv, "0"));
                let id: u32 = k.parse().unwrap_or(0);
                let v: i64 = v.parse().unwrap_or(0);
                if let Some(r) = face.find_fref(id) {
                    r.set_feature_value(v as u16, &mut feats);
                }
            }
        }
        let chars: Vec<u32> = f[4].chars().map(u32::from).collect();
        let dir: i32 = f[3].parse().unwrap_or(0);
        let Some(seg) = face.make_seg(None, Some(&feats), &chars, dir) else {
            writeln!(out, "T{task} noseg").unwrap();
            continue;
        };
        writeln!(
            out,
            "T{task} {} {:08x} {:08x}",
            seg.n_slots(),
            seg.advance_x().to_bits(),
            seg.advance_y().to_bits()
        )
        .unwrap();
        for s in seg.slots() {
            writeln!(
                out,
                " {} {:08x} {:08x} {:08x} {:08x} {} {} {}",
                seg.slot_gid(s),
                seg.slot_origin_x(s).to_bits(),
                seg.slot_origin_y(s).to_bits(),
                seg.slot_advance_x(s, None).to_bits(),
                seg.slot_advance_y(s, None).to_bits(),
                seg.slot_before(s),
                seg.slot_after(s),
                i32::from(seg.slot_can_insert_before(s))
            )
            .unwrap();
        }
    }
    std::io::stdout().write_all(out.as_bytes()).unwrap();
}
