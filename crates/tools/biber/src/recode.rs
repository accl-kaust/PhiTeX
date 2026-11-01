//! Biber::LaTeX::Recode `latex_decode`: LaTeX macros to Unicode with the
//! `base` character set of `recode_data.xml`. The data's regular
//! expressions are literal alternations, so each substitution is written
//! out as a scanner here, keeping Perl's match order (longest
//! alternative first, then backtracking into what follows).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use crate::unicode::{self, is_l, is_m, is_space, is_word};

static DATA: &str = include_str!("../data/recode_data.xml");

const TYPES: [&str; 9] = [
    "greek",
    "dings",
    "punctuation",
    "symbols",
    "negatedsymbols",
    "superscripts",
    "cmdsuperscripts",
    "letters",
    "diacritics",
];

pub struct Recode {
    maps: BTreeMap<&'static str, (BTreeMap<Vec<char>, String>, Vec<Vec<char>>)>,
}

impl Recode {
    pub fn new(set: &str) -> Recode {
        let doc = crate::xml::parse(DATA).expect("recode data");
        let mut maps: BTreeMap<&'static str, (BTreeMap<Vec<char>, String>, Vec<Vec<char>>)> =
            BTreeMap::new();
        let excl: Vec<String> = doc
            .kid("decode_exclude")
            .map(|d| d.kids("char").map(|c| unicode::nfd(&c.text)).collect())
            .unwrap_or_default();
        for t in TYPES {
            let mut m: BTreeMap<Vec<char>, String> = BTreeMap::new();
            let mut present = false;
            for maps_el in doc.kids("maps").filter(|e| e.attr("type") == Some(t)) {
                let sets = crate::config::split_comma(maps_el.attr("set").unwrap_or(""));
                if !sets.iter().any(|s| s == set) {
                    continue;
                }
                present = true;
                for map in maps_el.kids("map") {
                    let (Some(f), Some(to)) = (map.kid("from"), map.kid("to")) else {
                        continue;
                    };
                    m.insert(
                        unicode::nfd(&f.text).chars().collect(),
                        unicode::nfd(&to.text),
                    );
                }
            }
            for e in &excl {
                let k: Vec<char> = e.chars().collect();
                m.remove(&k);
            }
            if !present {
                continue;
            }
            let mut keys: Vec<Vec<char>> = m.keys().cloned().collect();
            keys.sort_by_key(|k| core::cmp::Reverse(k.len()));
            if keys.is_empty() {
                continue;
            }
            maps.insert(t, (m, keys));
        }
        Recode { maps }
    }

    pub fn decode(&self, text: &str) -> String {
        let mut t: Vec<char> = text.chars().collect();
        t = pre_box(&t);
        t = pre_char(&t);
        // (\\[a-zA-Z]+)\\(\s+) -> $1{}$2
        t = subst(&t, |t, p| {
            if t[p] != '\\' {
                return None;
            }
            let mut q = p + 1;
            while q < t.len() && t[q].is_ascii_alphabetic() {
                q += 1;
            }
            if q == p + 1 || q >= t.len() || t[q] != '\\' {
                return None;
            }
            let mut r = q + 1;
            while r < t.len() && is_space(t[r]) {
                r += 1;
            }
            if r == q + 1 {
                return None;
            }
            let mut o: String = t[p..q].iter().collect();
            o.push_str("{}");
            o.extend(&t[q + 1..r]);
            Some((r, o))
        });
        // ([^{]\\\w)([;,.:%]) -> $1{}$2
        t = subst(&t, |t, p| {
            if p + 3 < t.len()
                && t[p] != '{'
                && t[p + 1] == '\\'
                && is_word(t[p + 2])
                && matches!(t[p + 3], ';' | ',' | '.' | ':' | '%')
            {
                let mut o: String = t[p..p + 3].iter().collect();
                o.push_str("{}");
                o.push(t[p + 3]);
                return Some((p + 4, o));
            }
            None
        });
        for ty in TYPES {
            let Some((map, keys)) = self.maps.get(ty) else {
                continue;
            };
            let lit = |t: &[char], p: usize, k: &[char]| {
                t.len() >= p + k.len() && &t[p..p + k.len()] == k
            };
            match ty {
                "negatedsymbols" => {
                    t = subst(&t, |t, p| {
                        let pre: Vec<char> = "\\not\\".chars().collect();
                        if !lit(t, p, &pre) {
                            return None;
                        }
                        let q = p + pre.len();
                        keys.iter()
                            .find(|k| lit(t, q, k))
                            .map(|k| (q + k.len(), map[k].clone()))
                    });
                }
                "superscripts" | "cmdsuperscripts" => {
                    let pre: Vec<char> = if ty == "superscripts" {
                        "\\textsuperscript{"
                    } else {
                        "\\textsuperscript{\\"
                    }
                    .chars()
                    .collect();
                    t = subst(&t, |t, p| {
                        if !lit(t, p, &pre) {
                            return None;
                        }
                        let q = p + pre.len();
                        for k in keys {
                            if lit(t, q, k) && t.get(q + k.len()) == Some(&'}') {
                                return Some((q + k.len() + 1, map[k].clone()));
                            }
                        }
                        None
                    });
                }
                "dings" => {
                    let pre: Vec<char> = "\\ding{".chars().collect();
                    t = subst(&t, |t, p| {
                        if !lit(t, p, &pre) {
                            return None;
                        }
                        let q = p + pre.len();
                        let (a, b) = (*t.get(q)?, *t.get(q + 1)?);
                        if !(matches!(a, '2'..='9' | 'A' | 'F')
                            && (b.is_ascii_digit() || ('A'..='F').contains(&b)))
                        {
                            return None;
                        }
                        if t.get(q + 2) != Some(&'}') {
                            return None;
                        }
                        let k = alloc::vec![a, b];
                        Some((q + 3, map.get(&k).cloned().unwrap_or_default()))
                    });
                }
                "letters" | "punctuation" | "symbols" | "greek" => {
                    let space_brace = ty != "letters";
                    t = subst(&t, |t, p| {
                        if t[p] != '\\' {
                            return None;
                        }
                        let q = p + 1;
                        for k in keys {
                            if !lit(t, q, k) {
                                continue;
                            }
                            let e = q + k.len();
                            // (?:\{\}|\s+|\b)  or  (?: \{\}|\s+|\b)
                            let br: &[char] = if space_brace {
                                &[' ', '{', '}']
                            } else {
                                &['{', '}']
                            };
                            if lit(t, e, br) {
                                return Some((e + br.len(), map[k].clone()));
                            }
                            let mut r = e;
                            while r < t.len() && is_space(t[r]) {
                                r += 1;
                            }
                            if r > e {
                                return Some((r, map[k].clone()));
                            }
                            if word_boundary(t, e) {
                                return Some((e, map[k].clone()));
                            }
                        }
                        None
                    });
                }
                "diacritics" => {
                    t = Self::diacritics(&t, map, keys);
                }
                _ => {}
            }
        }
        // remove braces around a single grapheme not after \cmd{..}*
        let orig = t.clone();
        t = subst(&orig, |t, p| {
            if t[p] != '{' && t[p] != '\x1f' {
                return None;
            }
            let rest: String = t[p + 1..].iter().collect();
            let g = unicode::graphemes(&rest);
            let g0 = *g.first()?;
            let n = g0.chars().count();
            let c = *t.get(p + 1 + n)?;
            if c != '}' && c != '\x1e' {
                return None;
            }
            if preceded_by_macro(t, p) {
                return None;
            }
            Some((p + 2 + n, g0.into()))
        });
        let mut s: String = t.iter().collect();
        s = s
            .replace('\x1f', "{")
            .replace('\x1e', "}")
            .replace('\x0f', "{")
            .replace('\x0e', "}");
        s = s.replace("ı\u{301}", "i\u{301}");
        unicode::nfd(&s)
    }

    fn diacritics(t0: &[char], map: &BTreeMap<Vec<char>, String>, keys: &[Vec<char>]) -> Vec<char> {
        let lit =
            |t: &[char], p: usize, k: &[char]| t.len() >= p + k.len() && &t[p..p + k.len()] == k;
        // 1: {X} not after \<diacritic>  ->  \x0f X \x0e
        let t = subst(t0, |t, p| {
            if t[p] != '{' {
                return None;
            }
            let rest: String = t[p + 1..].iter().collect();
            let g = unicode::graphemes(&rest);
            let g0 = *g.first()?;
            let n = g0.chars().count();
            if t.get(p + 1 + n) != Some(&'}') {
                return None;
            }
            // preceded by \ + some key
            for k in keys {
                if p > k.len() && &t[p - k.len()..p] == k.as_slice() && t[p - k.len() - 1] == '\\' {
                    return None;
                }
            }
            let mut o = String::from("\x0f");
            o.push_str(g0);
            o.push('\x0e');
            Some((p + 2 + n, o))
        });
        // 2: (\{?)\\($re)\s*\{(\pL\pM*)\}(\}?)
        let t = subst(&t, |t, p| {
            for with_brace in [true, false] {
                let mut q = p;
                if with_brace {
                    if t[q] != '{' {
                        continue;
                    }
                    q += 1;
                }
                if t.get(q) != Some(&'\\') {
                    continue;
                }
                q += 1;
                for k in keys {
                    if !lit(t, q, k) {
                        continue;
                    }
                    let mut r = q + k.len();
                    while r < t.len() && is_space(t[r]) {
                        r += 1;
                    }
                    if t.get(r) != Some(&'{') {
                        continue;
                    }
                    let ls = r + 1;
                    if !t.get(ls).is_some_and(|&c| is_l(c)) {
                        continue;
                    }
                    let mut le = ls + 1;
                    while le < t.len() && is_m(t[le]) {
                        le += 1;
                    }
                    if t.get(le) != Some(&'}') {
                        continue;
                    }
                    let mut end = le + 1;
                    let close = t.get(end) == Some(&'}');
                    if close {
                        end += 1;
                    }
                    let mut o = String::new();
                    if with_brace {
                        o.push('\x1f');
                    }
                    o.extend(&t[ls..le]);
                    o.push_str(&map[k]);
                    if close {
                        o.push('\x1e');
                    }
                    return Some((end, o));
                }
            }
            None
        });
        // 3: (\{)(\pL\pM*)(\}) -> \x1f..\x1e
        let t = subst(&t, |t, p| {
            if t[p] != '{' || !t.get(p + 1).is_some_and(|&c| is_l(c)) {
                return None;
            }
            let mut e = p + 2;
            while e < t.len() && is_m(t[e]) {
                e += 1;
            }
            if t.get(e) != Some(&'}') {
                return None;
            }
            let mut o = String::from("\x1f");
            o.extend(&t[p + 1..e]);
            o.push('\x1e');
            Some((e + 1, o))
        });
        // 4: \\($re)(\s*)(cond \pM*)
        subst(&t, |t, p| {
            if t[p] != '\\' {
                return None;
            }
            let q = p + 1;
            for k in keys {
                if !lit(t, q, k) {
                    continue;
                }
                let e = q + k.len();
                let mut maxs = e;
                while maxs < t.len() && is_space(t[maxs]) {
                    maxs += 1;
                }
                let ends_letter = k.last().is_some_and(char::is_ascii_alphabetic);
                let mut s = maxs;
                loop {
                    let spaced = s > e;
                    if let Some(&c) = t.get(s) {
                        let ok = if !ends_letter || spaced {
                            is_l(c)
                        } else {
                            !c.is_ascii_alphabetic()
                        };
                        if ok {
                            let mut m = s + 1;
                            while m < t.len() && is_m(t[m]) {
                                m += 1;
                            }
                            let mut o: String = t[s..m].iter().collect();
                            o.push_str(&map[k]);
                            return Some((m, o));
                        }
                    }
                    if s == e {
                        break;
                    }
                    s -= 1;
                }
            }
            None
        })
    }
}

/// Is `p` just after something matching `\\\pL+(?:\{[^{]+\})*`.
fn preceded_by_macro(t: &[char], p: usize) -> bool {
    for k in 0..p {
        if t[k] != '\\' {
            continue;
        }
        let mut q = k + 1;
        while q < p && is_l(t[q]) {
            q += 1;
        }
        if q == k + 1 {
            continue;
        }
        // reachable ends: q, then groups
        let mut frontier = alloc::vec![q];
        let mut seen = alloc::collections::BTreeSet::new();
        while let Some(x) = frontier.pop() {
            if x == p {
                return true;
            }
            if !seen.insert(x) || x >= p {
                continue;
            }
            if t[x] != '{' {
                continue;
            }
            let mut r = x + 1;
            while r < p && t[r] != '{' {
                if t[r] == '}' && r > x + 1 {
                    frontier.push(r + 1);
                }
                r += 1;
            }
        }
    }
    false
}

fn word_boundary(t: &[char], p: usize) -> bool {
    let a = p > 0 && is_word(t[p - 1]);
    let b = p < t.len() && is_word(t[p]);
    a != b
}

/// Global substitution: at each position try `f`; on a match emit its
/// replacement and continue after it.
fn subst(t: &[char], f: impl Fn(&[char], usize) -> Option<(usize, String)>) -> Vec<char> {
    let mut o: Vec<char> = Vec::with_capacity(t.len());
    let mut p = 0;
    while p < t.len() {
        if let Some((e, r)) = f(t, p) {
            o.extend(r.chars());
            p = e;
        } else {
            o.push(t[p]);
            p += 1;
        }
    }
    o
}

/// `s/\\(h?box)\s+\{(\X)\}/\\$1\{$2\}/g`
fn pre_box(t: &[char]) -> Vec<char> {
    subst(t, |t, p| {
        if t[p] != '\\' {
            return None;
        }
        let mut q = p + 1;
        if t.get(q) == Some(&'h') {
            q += 1;
        }
        if !(t.get(q) == Some(&'b') && t.get(q + 1) == Some(&'o') && t.get(q + 2) == Some(&'x')) {
            return None;
        }
        let name_end = q + 3;
        let mut r = name_end;
        while r < t.len() && is_space(t[r]) {
            r += 1;
        }
        if r == name_end || t.get(r) != Some(&'{') {
            return None;
        }
        let rest: String = t[r + 1..].iter().collect();
        let g = unicode::graphemes(&rest);
        let g0 = *g.first()?;
        let n = g0.chars().count();
        if t.get(r + 1 + n) != Some(&'}') {
            return None;
        }
        let mut o: String = t[p..name_end].iter().collect();
        o.push('{');
        o.push_str(g0);
        o.push('}');
        Some((r + 2 + n, o))
    })
}

/// `\char"hex`, `\char'oct`, `\char dec`.
fn pre_char(t: &[char]) -> Vec<char> {
    let pre: Vec<char> = "\\char".chars().collect();
    let pass = |t: &[char], mode: u8| -> Vec<char> {
        subst(t, |t, p| {
            if !(t.len() >= p + 5 && t[p..p + 5] == pre[..]) {
                return None;
            }
            let mut q = p + 5;
            match mode {
                0 => {
                    if t.get(q) != Some(&'"') {
                        return None;
                    }
                    q += 1;
                    let s = q;
                    while q < t.len() && t[q].is_ascii_hexdigit() {
                        q += 1;
                    }
                    if q == s {
                        return None;
                    }
                    let h: String = t[s..q].iter().collect();
                    let v = u32::from_str_radix(&h, 16).ok()?;
                    Some((q, char::from_u32(v).map(String::from).unwrap_or_default()))
                }
                1 => {
                    if t.get(q) != Some(&'\'') {
                        return None;
                    }
                    q += 1;
                    let s = q;
                    while q < t.len() && unicode::is_digit(t[q]) {
                        q += 1;
                    }
                    if q == s {
                        return None;
                    }
                    let mut v: u32 = 0;
                    for c in &t[s..q] {
                        match c.to_digit(8) {
                            Some(d) => v = v * 8 + d,
                            None => break,
                        }
                    }
                    Some((q, char::from_u32(v).map(String::from).unwrap_or_default()))
                }
                _ => {
                    let s = q;
                    while q < t.len() && unicode::is_digit(t[q]) {
                        q += 1;
                    }
                    if q == s {
                        return None;
                    }
                    let d: String = t[s..q].iter().collect();
                    let v = unicode::num(&d).and_then(|v| u32::try_from(v).ok())?;
                    Some((q, char::from_u32(v).map(String::from).unwrap_or_default()))
                }
            }
        })
    };
    let a = pass(t, 0);
    let b = pass(&a, 1);
    pass(&b, 2)
}

/// The types `latex_encode` recodes, in its order.
const ENCODE_TYPES: [&str; 9] = [
    "greek",
    "dings",
    "negatedsymbols",
    "superscripts",
    "cmdsuperscripts",
    "diacritics",
    "letters",
    "punctuation",
    "symbols",
];

/// Biber::LaTeX::Recode `latex_encode`: Unicode to LaTeX macros with an
/// encode set of `recode_data.xml` (`output_safecharsset`), as biber
/// writes a `.bbl` in an encoding a character is not in. Each type's
/// map is its `to` → `from` (a `preferred` `from` winning), less
/// `encode_exclude`; its alternation tries the keys in sorted order.
pub struct Encoder {
    maps: BTreeMap<&'static str, (BTreeMap<Vec<char>, String>, Vec<Vec<char>>)>,
    /// The characters written as their `from` alone (`raw`).
    raw: BTreeSet<Vec<char>>,
    null: bool,
}

impl Encoder {
    pub fn new(set: &str) -> Encoder {
        let doc = crate::xml::parse(DATA).expect("recode data");
        let excl: Vec<Vec<char>> = doc
            .kid("encode_exclude")
            .map(|d| {
                d.kids("char")
                    .map(|c| unicode::nfd(&c.text).chars().collect())
                    .collect()
            })
            .unwrap_or_default();
        let mut maps = BTreeMap::new();
        let mut raw = BTreeSet::new();
        for t in ENCODE_TYPES {
            let mut m: BTreeMap<Vec<char>, String> = BTreeMap::new();
            let mut present = false;
            for maps_el in doc.kids("maps").filter(|e| e.attr("type") == Some(t)) {
                let sets = crate::config::split_comma(maps_el.attr("set").unwrap_or(""));
                if !sets.iter().any(|s| s == set) {
                    continue;
                }
                present = true;
                let pairs = || {
                    maps_el
                        .kids("map")
                        .filter_map(|map| Some((map.kid("from")?, map.kid("to")?)))
                };
                for (f, to) in pairs() {
                    m.insert(
                        unicode::nfd(&to.text).chars().collect(),
                        unicode::nfd(&f.text),
                    );
                }
                for (f, to) in pairs().filter(|(f, _)| f.attr("preferred").is_some()) {
                    m.insert(
                        unicode::nfd(&to.text).chars().collect(),
                        unicode::nfd(&f.text),
                    );
                }
                for (_, to) in pairs().filter(|(f, _)| f.attr("raw").is_some()) {
                    raw.insert(unicode::nfd(&to.text).chars().collect());
                }
            }
            for e in &excl {
                m.remove(e);
            }
            if !present || m.is_empty() {
                continue;
            }
            let keys: Vec<Vec<char>> = m.keys().cloned().collect();
            maps.insert(t, (m, keys));
        }
        Encoder {
            maps,
            raw,
            null: set == "null",
        }
    }

    /// `latex_encode`.
    pub fn encode(&self, text: &str) -> String {
        if self.null {
            return text.into();
        }
        let mut t: Vec<char> = text.chars().collect();
        for ty in ENCODE_TYPES {
            let Some((map, keys)) = self.maps.get(ty) else {
                continue;
            };
            // (the alternation: the first key in sorted order there)
            let at = |t: &[char], p: usize| -> Option<(usize, String)> {
                keys.iter()
                    .find(|k| t.len() >= p + k.len() && t[p..p + k.len()] == k[..])
                    .map(|k| (p + k.len(), map[k].clone()))
            };
            let is_raw = |t: &[char], p: usize, e: usize| self.raw.contains(&t[p..e]);
            t = match ty {
                "negatedsymbols" => subst(&t, |t, p| {
                    at(t, p).map(|(e, v)| (e, alloc::format!("{{$\\not\\{v}$}}")))
                }),
                "superscripts" => subst(&t, |t, p| {
                    at(t, p).map(|(e, v)| (e, alloc::format!("\\textsuperscript{{{v}}}")))
                }),
                "cmdsuperscripts" => subst(&t, |t, p| {
                    at(t, p).map(|(e, v)| (e, alloc::format!("\\textsuperscript{{\\{v}}}")))
                }),
                "dings" => subst(&t, |t, p| {
                    at(t, p).map(|(e, v)| (e, alloc::format!("\\ding{{{v}}}")))
                }),
                "letters" => subst(&t, |t, p| {
                    at(t, p).map(|(e, v)| {
                        if is_raw(t, p, e) {
                            (e, v)
                        } else {
                            (e, alloc::format!("\\{v}{{}}"))
                        }
                    })
                }),
                "punctuation" | "symbols" | "greek" => subst(&t, |t, p| {
                    at(t, p).map(|(e, v)| {
                        // `_wrap`
                        if v.starts_with("text") || v.starts_with("guil") {
                            (e, alloc::format!("\\{v}{{}}"))
                        } else if is_raw(t, p, e) {
                            (e, v)
                        } else {
                            (e, alloc::format!("{{$\\{v}$}}"))
                        }
                    })
                }),
                "diacritics" => diacritics(&t, &at),
                _ => t,
            };
        }
        t.into_iter().collect()
    }
}

/// `latex_encode`'s diacritics: `at` the alternation of the marks.
fn diacritics(t: &[char], at: &dyn Fn(&[char], usize) -> Option<(usize, String)>) -> Vec<char> {
    // s/i($re)/"\\" . $map->{$1} . '{\i}'/ge
    let t = subst(t, |t, p| {
        if t[p] != 'i' {
            return None;
        }
        at(t, p + 1).map(|(e, v)| (e, alloc::format!("\\{v}{{\\i}}")))
    });
    // s/\{(\pL\pM*)\}($re)/"\\" . $map->{$2} . "{$1}"/ge
    let t = subst(&t, |t, p| {
        if t[p] != '{' || !t.get(p + 1).is_some_and(|&c| is_l(c)) {
            return None;
        }
        let mut q = p + 2;
        while q < t.len() && is_m(t[q]) {
            q += 1;
        }
        if t.get(q) != Some(&'}') {
            return None;
        }
        let base: String = t[p + 1..q].iter().collect();
        at(t, q + 1).map(|(e, v)| (e, alloc::format!("\\{v}{{{base}}}")))
    });
    // s/(\pL\pM*)($re)/"\\" . $map->{$2} . "{$1}"/ge (\pM* backtracking)
    let t = subst(&t, |t, p| {
        if !is_l(t[p]) {
            return None;
        }
        let mut q = p + 1;
        while q < t.len() && is_m(t[q]) {
            q += 1;
        }
        (p + 1..=q).rev().find_map(|k| {
            at(t, k).map(|(e, v)| {
                let base: String = t[p..k].iter().collect();
                (e, alloc::format!("\\{v}{{{base}}}"))
            })
        })
    });
    // (\PM)($re){3,2,1}: the innermost mark first
    let mut t = t;
    for n in [3usize, 2, 1] {
        t = subst(&t, |t, p| {
            if is_m(t[p]) {
                return None;
            }
            let mut q = p + 1;
            let mut vs = Vec::new();
            for _ in 0..n {
                let (e, v) = at(t, q)?;
                vs.push(v);
                q = e;
            }
            let mut o = alloc::format!("{}", t[p]);
            for v in vs {
                o = alloc::format!("\\{v}{{{o}}}");
            }
            Some((q, o))
        });
    }
    t
}

/// An encoding the native biber reads a data source in or writes the
/// `.bbl` in (Perl Encode's names, any case).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    /// `ascii`: read as UTF-8 (`preprocess_file`), written with what it
    /// lacks recoded into macros (`Output::bbl`).
    Ascii,
    /// ISO-8859-1, a byte a character.
    Latin1,
}

impl Encoding {
    pub fn of(name: &str) -> Option<Encoding> {
        match unicode::lc(name).as_str() {
            "utf-8" | "utf8" => Some(Encoding::Utf8),
            "ascii" | "us-ascii" => Some(Encoding::Ascii),
            "latin1" | "iso-8859-1" | "iso8859-1" | "l1" => Some(Encoding::Latin1),
            _ => None,
        }
    }

    /// A data source's bytes as text (`slurp_switchr`), `None` if they
    /// are not in the encoding.
    pub fn read(self, bytes: &[u8]) -> Option<String> {
        match self {
            Encoding::Utf8 | Encoding::Ascii => core::str::from_utf8(bytes).ok().map(Into::into),
            Encoding::Latin1 => Some(bytes.iter().map(|&b| char::from(b)).collect()),
        }
    }

    /// Whether `c` can be written in the encoding.
    pub fn has(self, c: char) -> bool {
        match self {
            Encoding::Utf8 => true,
            Encoding::Ascii => c.is_ascii(),
            Encoding::Latin1 => u32::from(c) < 256,
        }
    }

    /// `text` as the `.bbl`'s `:encoding()` layer writes it: a character
    /// the encoding lacks as Encode's `\x{....}` (`FB_PERLQQ`).
    pub fn write(self, text: &str) -> String {
        let mut o = String::with_capacity(text.len());
        for c in text.chars() {
            if self.has(c) {
                o.push(c);
            } else {
                o.push_str(&alloc::format!("\\x{{{:04x}}}", u32::from(c)));
            }
        }
        o
    }
}
