//! Unicode::Collate (1.31, DUCET 13.0.0) as biber uses it: sort keys for
//! the sorting templates (`getSortKey`, variable weighting
//! `non-ignorable`, input already NFD so `prenormalized`, level 4 or 2,
//! `upper_before_lower`) and the level-1 `viewSortKey` behind
//! `sortinithash` (default options: `shifted`, NFD).
//!
//! Locales: Unicode::Collate::Locale's (`_locale`, its `entry` tailorings
//! added to the DUCET, [`Table::tailor`]); a locale whose file sets more
//! than an `entry`, or a CJK one, is not supported.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::unicode;

static DUCET: &[u8] = include_bytes!("../data/ducet.bin");

/// Unicode::Collate::Locale's `entry` tailorings: `@NAME` lines, each
/// followed by its entry's lines (data/tailorings.txt).
static TAILORINGS: &str = include_str!("../data/tailorings.txt");

/// Unicode::Collate::Locale 1.31's `%LocaleFile` (the locale names it
/// knows, aliases and short file names): `(name, file)`.
const LOCALE_FILES: &[(&str, &str)] = &[
    ("af", "af"),
    ("ar", "ar"),
    ("as", "as"),
    ("az", "az"),
    ("be", "be"),
    ("bn", "bn"),
    ("ca", "ca"),
    ("cs", "cs"),
    ("cu", "cu"),
    ("cy", "cy"),
    ("da", "da"),
    ("dsb", "dsb"),
    ("ee", "ee"),
    ("eo", "eo"),
    ("es", "es"),
    ("et", "et"),
    ("fa", "fa"),
    ("fi", "fi"),
    ("fil", "fil"),
    ("fo", "fo"),
    ("gu", "gu"),
    ("ha", "ha"),
    ("haw", "haw"),
    ("he", "he"),
    ("hi", "hi"),
    ("hr", "hr"),
    ("hu", "hu"),
    ("hy", "hy"),
    ("ig", "ig"),
    ("is", "is"),
    ("ja", "ja"),
    ("kk", "kk"),
    ("kl", "kl"),
    ("kn", "kn"),
    ("ko", "ko"),
    ("kok", "kok"),
    ("lkt", "lkt"),
    ("ln", "ln"),
    ("lt", "lt"),
    ("lv", "lv"),
    ("mk", "mk"),
    ("ml", "ml"),
    ("mr", "mr"),
    ("mt", "mt"),
    ("nb", "nb"),
    ("nn", "nn"),
    ("nso", "nso"),
    ("om", "om"),
    ("or", "or"),
    ("pa", "pa"),
    ("pl", "pl"),
    ("ro", "ro"),
    ("sa", "sa"),
    ("se", "se"),
    ("si", "si"),
    ("sk", "sk"),
    ("sl", "sl"),
    ("sq", "sq"),
    ("sr", "sr"),
    ("sv", "sv"),
    ("ta", "ta"),
    ("te", "te"),
    ("th", "th"),
    ("tn", "tn"),
    ("to", "to"),
    ("tr", "tr"),
    ("uk", "uk"),
    ("ur", "ur"),
    ("vi", "vi"),
    ("vo", "vo"),
    ("wae", "wae"),
    ("wo", "wo"),
    ("yo", "yo"),
    ("zh", "zh"),
    ("default", ""),
    ("bs", "hr"),
    ("bs_Cyrl", "sr"),
    ("sr_Latn", "hr"),
    ("de__phonebook", "de_phone"),
    ("de_AT_phonebook", "de_at_ph"),
    ("es__traditional", "es_trad"),
    ("fr_CA", "fr_ca"),
    ("fi__phonebook", "fi_phone"),
    ("si__dictionary", "si_dict"),
    ("sv__reformed", "sv_refo"),
    ("ug_Cyrl", "ug_cyrl"),
    ("zh__big5han", "zh_big5"),
    ("zh__gb2312han", "zh_gb"),
    ("zh__pinyin", "zh_pin"),
    ("zh__stroke", "zh_strk"),
    ("zh__zhuyin", "zh_zhu"),
];

fn locale_file(name: &str) -> Option<&'static str> {
    LOCALE_FILES.iter().find(|e| e.0 == name).map(|e| e.1)
}

/// Unicode::Collate::Locale's `_locale`: the locale name it accepts for
/// `l` (lower-cased, `-`, space and `.` made `_`, a type alias spelled
/// out; then the name, else its language, script, region and variant
/// tried from the longest), `default` where none is known.
#[must_use]
pub fn accepted_locale(l: &str) -> String {
    if l.is_empty() || l == "0" {
        return "default".into();
    }
    let mut loc: String = l
        .to_lowercase()
        .chars()
        .map(|c| if matches!(c, '-' | ' ' | '.') { '_' } else { c })
        .collect();
    if let Some((head, last)) = loc.rsplit_once('_')
        && !last.is_empty()
        && last.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        let alias = match last {
            "phone" | "phonebk" => Some("phonebook"),
            "dict" => Some("dictionary"),
            "reform" => Some("reformed"),
            "trad" => Some("traditional"),
            "big5" => Some("big5han"),
            "gb2312" => Some("gb2312han"),
            _ => None,
        };
        if let Some(a) = alias {
            loc = alloc::format!("{head}_{a}");
        }
    }
    if locale_file(&loc).is_some() {
        return loc;
    }
    let mut code = loc.split('_');
    let lan = code.next().unwrap_or("");
    let mut rest: Vec<&str> = code.collect();
    let scr = if rest.first().is_some_and(|c| c.len() == 4) {
        let c = rest.remove(0);
        let mut u: String = c[..1].to_uppercase();
        u.push_str(&c[1..]);
        u
    } else {
        String::new()
    };
    let reg = if rest.first().is_some_and(|c| c.len() < 4) {
        rest.remove(0).to_uppercase()
    } else {
        String::new()
    };
    let var = if rest.is_empty() { "" } else { rest.remove(0) };
    let mut list = Vec::new();
    if !var.is_empty() {
        list.push(alloc::format!("{lan}_{scr}_{reg}_{var}"));
        list.push(alloc::format!("{lan}_{scr}__{var}"));
        list.push(alloc::format!("{lan}_{reg}_{var}"));
        list.push(alloc::format!("{lan}__{var}"));
    }
    list.push(alloc::format!("{lan}_{scr}_{reg}"));
    list.push(alloc::format!("{lan}_{scr}"));
    list.push(alloc::format!("{lan}_{reg}"));
    list.push(lan.into());
    for l in list {
        if locale_file(&l).is_some() {
            return l;
        }
    }
    "default".into()
}

/// The `entry` tailoring of accepted locale `accepted`: `Some("")` for
/// `default` (none), `None` where this crate does not have its file's
/// tailoring (more than an `entry`, or a CJK one).
#[must_use]
pub fn tailoring(accepted: &str) -> Option<&'static str> {
    let file = locale_file(accepted)?;
    if file.is_empty() {
        return Some("");
    }
    let head = alloc::format!("@{file}\n");
    let at = if TAILORINGS.starts_with(&head[..]) {
        0
    } else {
        TAILORINGS.find(&alloc::format!("\n{head}"))? + 1
    };
    let body = &TAILORINGS[at + head.len()..];
    Some(body.find("\n@").map_or(body, |e| &body[..=e]))
}

#[derive(Clone, Copy, Debug)]
struct Ce {
    var: bool,
    w: [u16; 3],
}

#[derive(Clone)]
pub struct Table {
    map: BTreeMap<Vec<u32>, Vec<Ce>>,
    /// Longest contraction starting with a code point.
    maxlen: BTreeMap<u32, usize>,
    /// Prefixes of contractions longer than two (for discontiguous
    /// matching).
    contraction: alloc::collections::BTreeSet<Vec<u32>>,
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
        let mut contraction = alloc::collections::BTreeSet::new();
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
                    contraction.insert(u.clone());
                }
            }
            map.insert(cps, ces);
        }
        Table {
            map,
            maxlen,
            contraction,
        }
    }

    /// Unicode::Collate's `entry` (`parseEntry`): each line `CODES ;
    /// [.P.S.T]...` maps the code points to its collation elements (`*`
    /// for `.` marking a variable one), in the place of the DUCET's.
    pub fn tailor(&mut self, entry: &str) {
        for line in entry.lines() {
            let line = line.split('#').next().unwrap_or("");
            let Some((cps, ces)) = line.split_once(';') else {
                continue;
            };
            let cps: Vec<u32> = cps
                .split_whitespace()
                .filter_map(|h| u32::from_str_radix(h, 16).ok())
                .collect();
            if cps.is_empty() {
                continue;
            }
            let mut out = Vec::new();
            let mut l3_ignorable = true;
            for ce in ces.split('[').skip(1) {
                let ce = ce.split(']').next().unwrap_or("");
                let var = ce.starts_with('*');
                let ws: Vec<u16> = ce
                    .trim_start_matches(['.', '*'])
                    .split('.')
                    .map(|h| u16::from_str_radix(h, 16).unwrap_or(0))
                    .collect();
                let w = [
                    ws.first().copied().unwrap_or(0),
                    ws.get(1).copied().unwrap_or(0),
                    ws.get(2).copied().unwrap_or(0),
                ];
                if w != [0, 0, 0] {
                    l3_ignorable = false;
                }
                out.push(Ce { var, w });
            }
            if l3_ignorable {
                out.clear();
            }
            if cps.len() > 1 {
                let e = self.maxlen.entry(cps[0]).or_insert(0);
                if *e < cps.len() {
                    *e = cps.len();
                }
                let mut u = cps.clone();
                while u.len() > 2 {
                    u.pop();
                    self.contraction.insert(u.clone());
                }
            }
            self.map.insert(cps, out);
        }
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
            0xFA0E
                | 0xFA0F
                | 0xFA11
                | 0xFA13
                | 0xFA14
                | 0xFA1F
                | 0xFA21
                | 0xFA23
                | 0xFA24
                | 0xFA27
                | 0xFA28
                | 0xFA29
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
        Ce {
            var: false,
            w: [aaaa as u16, 0x20, 0x02],
        },
        Ce {
            var: false,
            w: [bbbb as u16, 0, 0],
        },
    ]
}

impl Collator {
    fn split(self, t: &Table, src: &mut [Option<u32>]) -> Vec<Vec<u32>> {
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
                            jcps.clone_from(&temp);
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
    pub fn sort_key(self, t: &Table, s: &str) -> Vec<u16> {
        let s = if self.normalize {
            unicode::nfd(s)
        } else {
            s.into()
        };
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
                        (
                            false,
                            [ce.w[0], ce.w[1], ce.w[2], if any { 0xFFFF } else { 0 }],
                        )
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
    pub fn view_sort_key(self, t: &Table, s: &str) -> String {
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
