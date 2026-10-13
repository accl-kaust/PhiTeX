//! Unicode::Collate (1.31, DUCET 13.0.0) as biber uses it: sort keys for
//! the sorting templates (`getSortKey`, variable weighting
//! `non-ignorable`, input already NFD so `prenormalized`, level 4 or 2,
//! `upper_before_lower`) and the level-1 `viewSortKey` behind
//! `sortinithash` (default options: `shifted`, NFD).
//!
//! Only untailored locales: Unicode::Collate::Locale has no tailoring for
//! them, so they collate with the plain DUCET.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::unicode;

static DUCET: &[u8] = include_bytes!("../data/ducet.bin");

#[derive(Clone, Copy, Debug)]
struct Ce {
    var: bool,
    w: [u16; 3],
}

pub struct Table {
    map: BTreeMap<Vec<u32>, Vec<Ce>>,
    /// Longest contraction starting with a code point.
    maxlen: BTreeMap<u32, usize>,
    /// Prefixes of contractions longer than two (for discontiguous
    /// matching).
    contraction: BTreeMap<Vec<u32>, ()>,
}

fn rd_u32(b: &[u8], i: &mut usize) -> u32 {
    let v = u32::from_le_bytes([b[*i], b[*i + 1], b[*i + 2], b[*i + 3]]);
    *i += 4;
    v
}
fn rd_u16(b: &[u8], i: &mut usize) -> u16 {
    let v = u16::from_le_bytes([b[*i], b[*i + 1]]);
    *i += 2;
    v
}

impl Table {
    pub fn load() -> Table {
        let b = DUCET;
        let mut i = 4;
        let vl = b[i] as usize;
        i += 1 + vl;
        let n = rd_u32(b, &mut i);
        let mut map = BTreeMap::new();
        let mut maxlen = BTreeMap::new();
        let mut contraction = BTreeMap::new();
        for _ in 0..n {
            let ncp = b[i] as usize;
            i += 1;
            let mut cps = Vec::with_capacity(ncp);
            for _ in 0..ncp {
                cps.push(rd_u32(b, &mut i));
            }
            let nce = b[i] as usize;
            i += 1;
            let mut ces = Vec::with_capacity(nce);
            let mut l3_ignorable = true;
            for _ in 0..nce {
                let var = b[i] != 0;
                i += 1;
                let p = rd_u16(b, &mut i);
                let s = rd_u16(b, &mut i);
                let t = rd_u16(b, &mut i);
                if p != 0 || s != 0 || t != 0 {
                    l3_ignorable = false;
                }
                ces.push(Ce { var, w: [p, s, t] });
            }
            if l3_ignorable {
                ces.clear();
            }
            if cps.len() > 1 {
                let e = maxlen.entry(cps[0]).or_insert(0);
                if *e < cps.len() {
                    *e = cps.len();
                }
                let mut u = cps.clone();
                while u.len() > 2 {
                    u.pop();
                    contraction.insert(u.clone(), ());
                }
            }
            map.insert(cps, ces);
        }
        Table { map, maxlen, contraction }
    }
}

/// A collator's settings.
#[derive(Clone, Copy, Debug)]
pub struct Collator {
    pub level: u8,
    pub upper_before_lower: bool,
    /// `variable => 'shifted'` (else non-ignorable).
    pub shifted: bool,
    /// Input normalised to NFD first (else `prenormalized`).
    pub normalize: bool,
}

const HANGUL_SBASE: u32 = 0xAC00;
const HANGUL_SCOUNT: u32 = 11172;

fn is_uideo(u: u32) -> bool {
    (0x4E00..=0x9FFC).contains(&u)
        || (0x3400..=0x4DBF).contains(&u)
        || (0x20000..=0x2A6DD).contains(&u)
        || (0x2A700..=0x2B734).contains(&u)
        || (0x2B740..=0x2B81D).contains(&u)
        || (0x2B820..=0x2CEA1).contains(&u)
        || (0x2CEB0..=0x2EBE0).contains(&u)
        || (0x30000..=0x3134A).contains(&u)
        || matches!(
            u,
            0xFA0E | 0xFA0F | 0xFA11 | 0xFA13 | 0xFA14 | 0xFA1F | 0xFA21 | 0xFA23 | 0xFA24 | 0xFA27 | 0xFA28 | 0xFA29
        )
}

/// UCA implicit weights (DUCET 13.0.0's `@implicitweights` and the
/// ideograph bases).
fn derived(u: u32) -> [Ce; 2] {
    let (aaaa, bbbb) = if (0x17000..=0x18AFF).contains(&u) || (0x18D00..=0x18D8F).contains(&u) {
        (0xFB00, (u - 0x17000) | 0x8000)
    } else if (0x1B170..=0x1B2FF).contains(&u) {
        (0xFB01, (u - 0x1B170) | 0x8000)
    } else if (0x18B00..=0x18CFF).contains(&u) {
        (0xFB02, (u - 0x18B00) | 0x8000)
    } else {
        let base = if is_uideo(u) {
            if (0x4E00..=0x9FFF).contains(&u) || (0xF900..=0xFAFF).contains(&u) {
                0xFB40
            } else {
                0xFB80
            }
        } else {
            0xFBC0
        };
        (base + (u >> 15), (u & 0x7FFF) | 0x8000)
    };
    [
        Ce { var: false, w: [aaaa as u16, 0x20, 0x02] },
        Ce { var: false, w: [bbbb as u16, 0, 0] },
    ]
}

impl Collator {
    fn split(&self, t: &Table, src: &mut Vec<Option<u32>>) -> Vec<Vec<u32>> {
        let mut buf = Vec::new();
        let mut i = 0;
        while i < src.len() {
            let Some(c0) = src[i] else {
                i += 1;
                continue;
            };
            let mut jcps = alloc::vec![c0];
            if let Some(&max) = t.maxlen.get(&c0) {
                let mut temp = jcps.clone();
                let mut len = 1;
                let mut p = i + 1;
                while len < max && p < src.len() {
                    if let Some(cp) = src[p] {
                        temp.push(cp);
                        len += 1;
                        if t.map.contains_key(&temp) {
                            jcps = temp.clone();
                            i = p;
                        }
                    }
                    p += 1;
                }
                if self.normalize {
                    let mut pre_cc = 0u8;
                    let mut out = Vec::new();
                    let mut p = i + 1;
                    while p < src.len() {
                        let Some(cp) = src[p] else {
                            p += 1;
                            continue;
                        };
                        let cur_cc = char::from_u32(cp).map_or(0, unicode::ccc);
                        if cur_cc == 0 {
                            break;
                        }
                        let mut tail = jcps.clone();
                        tail.push(cp);
                        if pre_cc != cur_cc && t.map.contains_key(&tail) {
                            jcps = tail;
                            out.push(p);
                        } else {
                            pre_cc = cur_cc;
                        }
                        p += 1;
                    }
                    for p in out {
                        src[p] = None;
                    }
                }
            }
            let ignorable = t.map.get(&jcps).is_some_and(Vec::is_empty);
            if !ignorable {
                buf.push(jcps);
            }
            i += 1;
        }
        let _ = &t.contraction;
        buf
    }

    fn weights(t: &Table, jcps: &[u32]) -> Vec<Ce> {
        if let Some(ce) = t.map.get(jcps) {
            return ce.clone();
        }
        let u = jcps[0];
        if (HANGUL_SBASE..HANGUL_SBASE + HANGUL_SCOUNT).contains(&u) {
            let s = u - HANGUL_SBASE;
            let l = 0x1100 + s / 588;
            let v = 0x1161 + (s % 588) / 28;
            let tt = 0x11A7 + s % 28;
            let mut dec: Vec<Vec<u32>> = if tt == 0x11A7 {
                alloc::vec![alloc::vec![l], alloc::vec![v]]
            } else {
                alloc::vec![alloc::vec![l], alloc::vec![v], alloc::vec![tt]]
            };
            if dec.len() == 2 {
                let c = alloc::vec![l, v];
                if t.map.contains_key(&c) {
                    dec = alloc::vec![c];
                }
            } else {
                if t.maxlen.contains_key(&l) {
                    let c = alloc::vec![l, v, tt];
                    if t.map.contains_key(&c) {
                        dec = alloc::vec![c];
                    } else {
                        let c = alloc::vec![l, v];
                        if t.map.contains_key(&c) {
                            dec = alloc::vec![c, alloc::vec![tt]];
                        }
                    }
                }
                if dec.len() == 3 && t.maxlen.contains_key(&v) {
                    let c = alloc::vec![v, tt];
                    if t.map.contains_key(&c) {
                        dec = alloc::vec![alloc::vec![l], c];
                    }
                }
            }
            let mut out = Vec::new();
            for d in dec {
                match t.map.get(&d) {
                    Some(ce) => out.extend_from_slice(ce),
                    None => out.extend_from_slice(&derived(d[0])),
                }
            }
            return out;
        }
        let u = if u > 0x10FFFF { 0xFFFD } else { u };
        derived(u).to_vec()
    }

    /// The sort key as Unicode::Collate builds it: the four levels of
    /// 16-bit weights, each level ended by a zero separator.
    pub fn sort_key(&self, t: &Table, s: &str) -> Vec<u16> {
        let s = if self.normalize { unicode::nfd(s) } else { s.into() };
        let mut src: Vec<Option<u32>> = s.chars().map(|c| Some(c as u32)).collect();
        let ents = self.split(t, &mut src);
        let mut levels: [Vec<u16>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        let mut last_is_variable = false;
        for j in &ents {
            for ce in Self::weights(t, j) {
                // varCE
                let (var, w4) = if self.shifted {
                    if ce.var {
                        (true, [0u16, 0, 0, ce.w[0]])
                    } else {
                        let any = ce.w[0] != 0 || ce.w[1] != 0 || ce.w[2] != 0;
                        (false, [ce.w[0], ce.w[1], ce.w[2], if any { 0xFFFF } else { 0 }])
                    }
                } else {
                    (ce.var, [ce.w[0], ce.w[1], ce.w[2], 0])
                };
                if self.shifted {
                    // "ignorable after variable"
                    if var {
                        last_is_variable = true;
                    } else if w4[0] == 0 {
                        if last_is_variable {
                            continue;
                        }
                    } else {
                        last_is_variable = false;
                    }
                }
                for (v, lv) in levels.iter_mut().enumerate().take(self.level as usize) {
                    if w4[v] > 0 {
                        lv.push(w4[v]);
                    }
                }
            }
        }
        if self.upper_before_lower {
            for w in &mut levels[2] {
                if (0x8..=0xC).contains(w) {
                    *w -= 6;
                } else if (0x2..=0x6).contains(w) {
                    *w += 6;
                } else if *w == 0x1C {
                    *w += 1;
                } else if *w == 0x1D {
                    *w -= 1;
                }
            }
        }
        let mut key = Vec::new();
        for (i, l) in levels.iter().enumerate() {
            if i > 0 {
                key.push(0);
            }
            key.extend_from_slice(l);
        }
        if self.level == 4 {
            key.push(0);
        }
        key
    }

    /// `viewSortKey`.
    pub fn view_sort_key(&self, t: &Table, s: &str) -> String {
        let key = self.sort_key(t, s);
        let mut o = String::from("[");
        let mut sep = 0;
        for &w in &key {
            if o.as_bytes().last() != Some(&b'[') {
                o.push(' ');
            }
            if w != 0 || sep >= 4 {
                o.push_str(&alloc::format!("{w:04X}"));
            } else {
                o.push('|');
                sep += 1;
            }
        }
        o.push(']');
        o
    }
}

/// Compare two sort keys as Perl compares the packed byte strings.
pub fn cmp_keys(a: &[u16], b: &[u16]) -> core::cmp::Ordering {
    a.cmp(b)
}
