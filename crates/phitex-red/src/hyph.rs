//! C4: `\patterns`, `\hyphenation`, and hyphenating a word (tex.web 891–965).
//!
//! Patterns and exceptions are persistent values (`Val::Patterns` /
//! `Val::Exceptions` under `Key::Patterns(lang)` / `Key::Exceptions(lang)`),
//! always defined globally, as TeX's trie and hyphenation table are global.
//! The trie is not packed: a pattern is a map entry from its letter string
//! (hyph codes, 0 for the `.` edge marker) to its inter-letter values; the
//! matching is tex.web 923 over that map, which gives the same `hyf` array.
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::ctx::Ctx;
use crate::prim::{Kind, Registry, ip};
use crate::tok::{Tok, Toks};
use crate::val::{Key, Meaning, Val};

/// TeX's limit on letters in a pattern or exception word (tex.web 962, 937).
const MAX_LEN: usize = 63;

pub fn register(r: &mut Registry) {
    r.add("patterns", 1, Kind::Command(new_patterns), 0);
    r.add("hyphenation", 0, Kind::Command(new_hyph_exceptions), 0);
}

/// One language's patterns. Cheap to clone; `\patterns` makes a new value.
#[derive(Clone, Debug, Default)]
pub struct Patterns {
    /// Letter string (`hc[1..k]`, 0 = word edge) → values `hyf[0..=k]`
    /// (`v[i]` sits between letter `i-1` and letter `i`). Only patterns
    /// with some nonzero value are kept (TeX stores no op for the others).
    pats: Arc<BTreeMap<Box<[u16]>, Box<[u8]>>>,
    /// The longest key.
    max_len: usize,
    /// e-TeX `\savinghyphcodes`: the `\lccode`s (0..=255) saved with the patterns.
    codes: Option<Arc<[u16]>>,
}

impl PartialEq for Patterns {
    fn eq(&self, o: &Self) -> bool {
        (Arc::ptr_eq(&self.pats, &o.pats) || self.pats == o.pats) && self.codes == o.codes
    }
}

impl Patterns {
    pub fn is_empty(&self) -> bool {
        self.pats.is_empty()
    }
    /// e-TeX's hyph codes stored with these patterns, if any (256 entries).
    pub fn codes(&self) -> Option<&[u16]> {
        self.codes.as_deref()
    }
    /// The hyph code of `c` when codes were saved (`set_lc_code` with a
    /// nonzero `hyph_index`): `None` means use `\lccode`.
    pub fn hyph_code(&self, c: u16) -> Option<u16> {
        self.codes.as_ref().map(|t| t.get(c as usize).copied().unwrap_or(0))
    }
    /// Inserts one pattern; returns true if it duplicates a stored one (TeX's
    /// "Duplicate pattern": the new values replace the old).
    pub fn insert(&mut self, letters: &[u16], values: &[u8]) -> bool {
        if values.iter().all(|&v| v == 0) {
            return false;
        }
        let m = Arc::make_mut(&mut self.pats);
        self.max_len = self.max_len.max(letters.len());
        m.insert(letters.into(), values.into()).is_some()
    }
    pub fn set_codes(&mut self, codes: Arc<[u16]>) {
        self.codes = Some(codes);
    }
}

/// One language's `\hyphenation` exceptions. Cheap to clone.
#[derive(Clone, Debug, Default)]
pub struct Exceptions {
    /// Word (hyph codes) → hyphen positions (after that many letters), sorted.
    words: Arc<BTreeMap<Box<[u16]>, Arc<[u8]>>>,
}

impl PartialEq for Exceptions {
    fn eq(&self, o: &Self) -> bool {
        Arc::ptr_eq(&self.words, &o.words) || self.words == o.words
    }
}

impl Exceptions {
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }
    /// Enters a word; an entry for the same word is replaced (TeX 3.14159+).
    pub fn insert(&mut self, word: &[u16], mut pos: Vec<u8>) {
        pos.sort_unstable();
        pos.dedup();
        Arc::make_mut(&mut self.words).insert(word.into(), pos.into());
    }
    pub fn get(&self, word: &[u16]) -> Option<&[u8]> {
        self.words.get(word).map(|p| &**p)
    }
}

/// `norm_min` (tex.web 1091).
pub fn norm_min(h: i32) -> usize {
    h.clamp(1, 63) as usize
}

/// `cur_lang` from `\language` (tex.web 934: out of 1..=255 is 0).
pub fn cur_lang(c: &mut dyn Ctx) -> u8 {
    let l = c.int(&Key::IntPar(ip::of("language")));
    if l <= 0 || l > 255 { 0 } else { l as u8 }
}

/// The patterns of a language (empty if none).
pub fn patterns_of(c: &mut dyn Ctx, lang: u8) -> Patterns {
    match c.get(&Key::Patterns(lang)) {
        Val::Patterns(p) => p,
        _ => Patterns::default(),
    }
}

/// The exceptions of a language (empty if none).
pub fn exceptions_of(c: &mut dyn Ctx, lang: u8) -> Exceptions {
    match c.get(&Key::Exceptions(lang)) {
        Val::Exceptions(e) => e,
        _ => Exceptions::default(),
    }
}

/// Hyphen positions of a word (lowercase codes), language data given.
///
/// A position `j` means a hyphen may follow the `j`-th letter (1-based),
/// i.e. it lies between `word[j-1]` and `word[j]`; `lhm <= j <= len - rhm`.
/// tex.web 923 (patterns), 930–931 (exceptions, which take precedence and
/// are subject to the same hyphen mins).
pub fn hyphenate(word: &[u16], p: Option<&Patterns>, e: Option<&Exceptions>, lhm: usize, rhm: usize) -> Vec<usize> {
    let lhm = lhm.clamp(1, 63);
    let rhm = rhm.clamp(1, 63);
    let hn = word.len();
    if hn < lhm + rhm {
        return Vec::new();
    }
    // hyf[t]: the value after letter t (0..=hn).
    let mut hyf = vec![0u8; hn + 1];
    if let Some(pos) = e.and_then(|e| e.get(word)) {
        for &q in pos {
            if (q as usize) <= hn {
                hyf[q as usize] = 1;
            }
        }
    } else {
        let Some(p) = p.filter(|p| !p.is_empty()) else { return Vec::new() };
        // hc[0..=hn+1]: edge, letters, edge.
        let mut w = Vec::with_capacity(hn + 2);
        w.push(0u16);
        w.extend_from_slice(word);
        w.push(0);
        for j in 0..w.len() {
            let most = p.max_len.min(w.len() - j);
            for l in 1..=most {
                let Some(v) = p.pats.get(&w[j..j + l]) else { continue };
                for (i, &d) in v.iter().enumerate() {
                    // value i sits after hc[j+i-1].
                    if j + i == 0 {
                        continue;
                    }
                    let t = j + i - 1;
                    if t <= hn && d > hyf[t] {
                        hyf[t] = d;
                    }
                }
            }
        }
    }
    (lhm..=hn - rhm).filter(|&j| hyf[j] & 1 == 1).collect()
}

/// What a token is to `\patterns`/`\hyphenation` (its command code).
enum Cls {
    Letter(u32),
    CharGiven(u32),
    CharNum,
    Space,
    RBrace,
    Other,
}

fn classify(c: &mut dyn Ctx, t: &Tok) -> Cls {
    let by_cat = |cat: u8, ch: char| match cat {
        11 | 12 => Cls::Letter(ch as u32),
        10 => Cls::Space,
        2 => Cls::RBrace,
        _ => Cls::Other,
    };
    match *t {
        Tok::Char { cat, ch } => by_cat(cat, ch),
        Tok::Cs(_) | Tok::Active(_) => match c.meaning(t) {
            Meaning::Char { cat, ch } => by_cat(cat, ch),
            Meaning::CharGiven(n) => Cls::CharGiven(n),
            Meaning::Prim(id) if crate::prim::registry().get(id).name == "char" => Cls::CharNum,
            _ => Cls::Other,
        },
        _ => Cls::Other,
    }
}

fn lc_code(c: &mut dyn Ctx, ch: u32) -> u16 {
    match char::from_u32(ch) {
        Some(x) => c.int(&Key::Lc(x)).clamp(0, i32::from(u16::MAX)) as u16,
        None => 0,
    }
}

const APPENDIX_H: &[&str] = &["(See Appendix H.)"];

/// `\afterassignment` after a `hyph_data` command (tex.web 1252 `goto done`).
fn after_assignment(c: &mut dyn Ctx) {
    let t = c.get(&Key::AfterAssignment).toks();
    if let Some(&tok) = t.first() {
        c.define(Key::AfterAssignment, Val::Toks(Toks::from(Vec::<Tok>::new())), true);
        c.back(tok);
    }
}

/// `new_patterns` (tex.web 960–965; e-TeX hyph codes).
fn new_patterns(c: &mut dyn Ctx, _code: u16) {
    let lang = cur_lang(c);
    let mut pats = patterns_of(c, lang);
    crate::scan::scan_left_brace(c);
    let mut k = 0usize;
    let mut hc = [0u16; MAX_LEN + 1];
    let mut hyf = [0u8; MAX_LEN + 1];
    let mut digit_sensed = false;
    loop {
        if c.halted() {
            return;
        }
        let Some(t) = c.next_x() else { break };
        match classify(c, &t) {
            Cls::Letter(ch) => {
                if digit_sensed || !(0x30..=0x39).contains(&ch) {
                    let code = if ch == u32::from(b'.') {
                        0
                    } else {
                        let l = lc_code(c, ch);
                        if l == 0 {
                            crate::diag::error(c, "Nonletter", APPENDIX_H);
                        }
                        l
                    };
                    if k < MAX_LEN {
                        k += 1;
                        hc[k] = code;
                        hyf[k] = 0;
                        digit_sensed = false;
                    }
                } else if k < MAX_LEN {
                    hyf[k] = (ch - 0x30) as u8;
                    digit_sensed = true;
                }
            }
            cls @ (Cls::Space | Cls::RBrace) => {
                if k > 0 {
                    if hc[1] == 0 {
                        hyf[0] = 0;
                    }
                    if hc[k] == 0 {
                        hyf[k] = 0;
                    }
                    if pats.insert(&hc[1..=k], &hyf[0..=k]) {
                        crate::diag::error(c, "Duplicate pattern", APPENDIX_H);
                    }
                }
                if matches!(cls, Cls::RBrace) {
                    break;
                }
                k = 0;
                hyf[0] = 0;
                digit_sensed = false;
            }
            _ => {
                crate::diag::error(c, "Bad \\patterns", APPENDIX_H);
            }
        }
    }
    // e-TeX: store hyphenation codes for the current language.
    if c.etex() && c.int(&Key::IntPar(ip::of("savinghyphcodes"))) > 0 {
        let codes: Vec<u16> = (0u32..256).map(|ch| lc_code(c, ch)).collect();
        pats.set_codes(codes.into());
    }
    c.define(Key::Patterns(lang), Val::Patterns(pats), true);
    after_assignment(c);
}

/// `new_hyph_exceptions` (tex.web 934–939; e-TeX `set_lc_code` via hyph codes).
fn new_hyph_exceptions(c: &mut dyn Ctx, _code: u16) {
    crate::scan::scan_left_brace(c);
    let lang = cur_lang(c);
    // e-TeX `set_hyph_index`: the language's saved codes, if any.
    let codes = if c.etex() { patterns_of(c, lang).codes.clone() } else { None };
    let mut exc = exceptions_of(c, lang);
    let mut word: Vec<u16> = Vec::new();
    let mut pos: Vec<u8> = Vec::new();
    loop {
        if c.halted() {
            return;
        }
        let Some(t) = c.next_x() else { break };
        let cls = classify(c, &t);
        let ch = match cls {
            Cls::Letter(ch) | Cls::CharGiven(ch) => Some(ch),
            Cls::CharNum => Some(u32::from(crate::scan::scan_char_num(c))),
            _ => None,
        };
        if let Some(ch) = ch {
            if ch == u32::from(b'-') {
                if word.len() < MAX_LEN {
                    pos.push(word.len() as u8);
                }
            } else {
                let code = match &codes {
                    Some(t) => t.get(ch as usize).copied().unwrap_or(0),
                    None => lc_code(c, ch),
                };
                if code == 0 {
                    crate::diag::error(
                        c,
                        "Not a letter",
                        &["Letters in \\hyphenation words must have \\lccode>0.", "Proceed; I'll ignore the character I just read."],
                    );
                } else if word.len() < MAX_LEN {
                    word.push(code);
                }
            }
            continue;
        }
        match cls {
            Cls::Space | Cls::RBrace => {
                if word.len() > 1 {
                    exc.insert(&word, std::mem::take(&mut pos));
                }
                if matches!(cls, Cls::RBrace) {
                    break;
                }
                word.clear();
                pos.clear();
            }
            _ => {
                crate::diag::error(
                    c,
                    "Improper \\hyphenation will be flushed",
                    &["Hyphenation exceptions must contain only letters", "and hyphens. But continue; I'll forgive and forget."],
                );
            }
        }
    }
    c.define(Key::Exceptions(lang), Val::Exceptions(exc), true);
    after_assignment(c);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.bytes().map(u16::from).collect()
    }
    /// Parses a pattern like "hy3ph" or ".ab1c" into (letters, values).
    fn pat(p: &mut Patterns, s: &str) {
        let mut hc = vec![];
        let mut hyf = vec![0u8];
        for b in s.bytes() {
            if b.is_ascii_digit() {
                *hyf.last_mut().unwrap() = b - b'0';
            } else {
                hc.push(if b == b'.' { 0 } else { u16::from(b) });
                hyf.push(0);
            }
        }
        p.insert(&hc, &hyf);
    }

    #[test]
    fn liang_basic() {
        let mut p = Patterns::default();
        for s in ["hy3ph", "he2n", "hena4", "hen5at", "1na", "n2at", "1tio", "2io", "o2n"] {
            pat(&mut p, s);
        }
        // hy-phen-ation
        assert_eq!(hyphenate(&w("hyphenation"), Some(&p), None, 2, 3), vec![2, 6]);
    }

    #[test]
    fn exceptions_win_and_respect_mins() {
        let mut e = Exceptions::default();
        e.insert(&w("table"), vec![1, 3]);
        assert_eq!(hyphenate(&w("table"), None, Some(&e), 2, 2), vec![3]);
        assert_eq!(hyphenate(&w("table"), None, Some(&e), 1, 2), vec![1, 3]);
    }
}
