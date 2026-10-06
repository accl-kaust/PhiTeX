//! ICU's line breaking, as `XeTeX` asks for it with
//! `\XeTeXlinebreaklocale` (`linebreak_start`, `linebreak_next` in
//! `XeTeX_ext.c`: `ubrk_open(UBRK_LINE, locale)`, then `ubrk_next` over the
//! word's UTF-16 text).
//!
//! ICU4C's rule-based break iterator, its forward run (`handleNext` in
//! `rbbi.cpp`), over ICU's own compiled rules: the state tables and
//! character classes of its line rule sets, read from the ICU `XeTeX` runs
//! with by `scripts/xetex/icu-linebreak.cpp` into `icu_linebreak.bin`.
//! The locale picks the rule set as ICU's break iterator data does
//! (`brkitr/{root,ja,ko,zh}.txt`, with the `lb` keyword).
//!
//! Not here: ICU's dictionaries, which split runs of Thai, Lao, Khmer and
//! Myanmar (the rules' dictionary classes) into words, and the
//! `lw=phrase` rule sets of Japanese and Korean (dictionaries again).
//! [`Rules::breaks`] says when it met the one, [`rule_set`] the other.

use alloc::vec::Vec;

/// ICU's line break rule sets, as `brkitr` names them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RuleSet {
    /// `line.brk`: root's `line` and `line_strict`.
    Line,
    /// `line_loose.brk`.
    Loose,
    /// `line_normal.brk`: also Japanese and Korean's `line`.
    Normal,
    /// `line_cj.brk`: Chinese's `line`, and `line_strict` in Chinese,
    /// Japanese and Korean.
    Cj,
    /// `line_loose_cj.brk`.
    LooseCj,
    /// `line_normal_cj.brk`.
    NormalCj,
}

/// The rule set ICU's `ubrk_open(UBRK_LINE, locale)` uses, from the
/// locale's language and its `lb` keyword (`@lb=…`, or `-u-lb-…`), the way
/// ICU's locale IDs and resource fallback find it. `None`: a phrase rule
/// set (`lw=phrase` in Japanese or Korean), not here.
///
/// A locale ICU has no break data for falls back to the default locale
/// (the environment's) before root: taken as one without its own rules
/// (English, C), as in TeX Live's runs.
pub(crate) fn rule_set(locale: &[u8]) -> Option<RuleSet> {
    let (base, keywords) = split_keywords(locale);
    // the language: up to the first separator; a POSIX `.charset` right
    // after it makes a name with no fallback to the language (root's)
    let end = base
        .iter()
        .position(|&b| matches!(b, b'_' | b'-' | b'.'))
        .unwrap_or(base.len());
    let lang: Vec<u8> = base[..end].to_ascii_lowercase();
    let data = match lang.as_slice() {
        _ if base.get(end) == Some(&b'.') => Data::Root,
        b"ja" | b"jpn" | b"ko" | b"kor" => Data::JaKo,
        b"zh" | b"zho" => Data::Zh,
        _ => Data::Root,
    };
    if data == Data::JaKo && keywords.phrase {
        return None;
    }
    Some(match (data, keywords.lb) {
        (Data::Root, Lb::None | Lb::Strict) => RuleSet::Line,
        (Data::Root, Lb::Loose) => RuleSet::Loose,
        (Data::Root, Lb::Normal) | (Data::JaKo, Lb::None) => RuleSet::Normal,
        (Data::JaKo | Data::Zh, Lb::Strict) | (Data::Zh, Lb::None) => RuleSet::Cj,
        (Data::JaKo | Data::Zh, Lb::Loose) => RuleSet::LooseCj,
        (Data::JaKo | Data::Zh, Lb::Normal) => RuleSet::NormalCj,
    })
}

/// The `brkitr` data a locale finds: root's (or a language's that has
/// root's line rules), Japanese and Korean's, Chinese's.
#[derive(PartialEq)]
enum Data {
    Root,
    JaKo,
    Zh,
}

/// The `lb` keyword.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lb {
    None,
    Strict,
    Normal,
    Loose,
}

/// The keywords of a locale that choose the line rules.
struct Keywords {
    lb: Lb,
    /// `lw=phrase`.
    phrase: bool,
}

/// A locale ID's base name and its `lb` and `lw` keywords: `base@k=v;k=v`
/// (keys in any case, values as written; the first of a key counts), or
/// BCP 47's `base-u-k-v`. A keyword without a value makes the ID one ICU
/// does not take: root's (`ja@lb`).
fn split_keywords(locale: &[u8]) -> (&[u8], Keywords) {
    let mut kw = Keywords {
        lb: Lb::None,
        phrase: false,
    };
    let (mut seen_lb, mut seen_lw) = (false, false);
    let mut set = |key: &[u8], value: &[u8]| {
        if key.eq_ignore_ascii_case(b"lb") && !seen_lb {
            seen_lb = true;
            kw.lb = match value {
                b"strict" => Lb::Strict,
                b"normal" => Lb::Normal,
                b"loose" => Lb::Loose,
                _ => Lb::None,
            };
        } else if key.eq_ignore_ascii_case(b"lw") && !seen_lw {
            seen_lw = true;
            kw.phrase = value == b"phrase";
        }
    };
    if let Some(at) = locale.iter().position(|&b| b == b'@') {
        for item in locale[at + 1..].split(|&b| b == b';') {
            let eq = item.iter().position(|&b| b == b'=');
            let (k, v) = match eq {
                Some(eq) => (item[..eq].trim_ascii(), item[eq + 1..].trim_ascii()),
                None => (item.trim_ascii(), &b""[..]),
            };
            if k.is_empty() || v.is_empty() {
                let none = Keywords {
                    lb: Lb::None,
                    phrase: false,
                };
                return (b"", none);
            }
            set(k, v);
        }
        return (&locale[..at], kw);
    }
    // BCP 47: the `u` extension's keys and values (a private use `x`
    // before it hides it)
    let subtags: Vec<&[u8]> = locale.split(|&b| b == b'-' || b == b'_').collect();
    let Some(u) = subtags.iter().position(|s| s.eq_ignore_ascii_case(b"u")) else {
        return (locale, kw);
    };
    if subtags[..u].iter().any(|s| s.eq_ignore_ascii_case(b"x")) {
        return (locale, kw);
    }
    let base_len: usize = subtags[..u].iter().map(|s| s.len() + 1).sum::<usize>();
    let mut i = u + 1;
    while i < subtags.len() && subtags[i].len() > 1 {
        let key = subtags[i];
        i += 1;
        let start = i;
        while i < subtags.len() && subtags[i].len() > 2 {
            i += 1;
        }
        if key.len() == 2
            && i > start
            && let Some(first) = subtags.get(start)
        {
            set(key, first);
        }
    }
    (&locale[..base_len.saturating_sub(1)], kw)
}

/// The compiled rules, all sets.
static DATA: &[u8] = include_bytes!("icu_linebreak.bin");

/// One rule set, ready to run: the forward state table and the character
/// classes.
pub(crate) struct Rules {
    /// The categories (columns of next states).
    cats: usize,
    /// The first dictionary category.
    dict_start: u16,
    /// Room for the look-ahead positions.
    lookahead: usize,
    /// `RBBI_BOF_REQUIRED`: a run starts on the begin-of-input category.
    bof: bool,
    /// The states' rows: accepting, look-ahead, then the next state by
    /// category.
    table: Vec<u16>,
    /// Each code point's category, by the start of its range.
    ranges: Vec<(u32, u8)>,
}

/// A reader of `DATA`.
struct Reader<'a> {
    d: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> u8 {
        let b = self.d.get(self.i).copied().unwrap_or(0);
        self.i += 1;
        b
    }
    fn u16(&mut self) -> u16 {
        u16::from(self.u8()) | (u16::from(self.u8()) << 8)
    }
    fn varint(&mut self) -> u32 {
        let (mut v, mut shift) = (0u32, 0);
        loop {
            let b = self.u8();
            v |= u32::from(b & 0x7f) << shift;
            shift += 7;
            if b & 0x80 == 0 || shift > 28 {
                return v;
            }
        }
    }
}

/// A table as read: its parameters and its rows.
struct TableData {
    cats: usize,
    dict_start: u16,
    lookahead: usize,
    flags: u8,
    table: Vec<u16>,
}

fn read_table(r: &mut Reader) -> TableData {
    let cats = usize::from(r.u8());
    let states = usize::from(r.u16());
    let dict_start = u16::from(r.u8());
    let lookahead = usize::from(r.u8());
    let flags = r.u8();
    let w = cats + 2;
    let mut table = alloc::vec![0u16; states * w];
    for s in 0..states {
        let base = usize::from(r.u16());
        if base != 0xFFFF {
            table.copy_within(base * w..(base + 1) * w, s * w);
        }
        for _ in 0..r.u8() {
            let col = usize::from(r.u8());
            let v = r.u16();
            if let Some(x) = table.get_mut(s * w + col) {
                *x = v;
            }
        }
    }
    TableData {
        cats,
        dict_start,
        lookahead,
        flags,
        table,
    }
}

fn read_ranges(r: &mut Reader) -> Vec<(u32, u8)> {
    let n = r.u16();
    let mut at = 0;
    (0..n)
        .map(|_| {
            at += r.varint();
            (at, r.u8())
        })
        .collect()
}

impl Rules {
    /// Rule set `set`, read from `DATA`.
    pub(crate) fn new(set: RuleSet) -> Self {
        let mut r = Reader { d: DATA, i: 4 };
        let tables: Vec<TableData> = (0..r.u8()).map(|_| read_table(&mut r)).collect();
        let ntries = r.u8();
        let first = read_ranges(&mut r);
        let mut tries = Vec::new();
        for _ in 1..ntries {
            let map: Vec<u8> = (0..r.u8()).map(|_| r.u8()).collect();
            let diffs = read_ranges(&mut r);
            tries.push(overlay(&first, &map, &diffs));
        }
        tries.insert(0, first);
        let sets: Vec<(u8, u8)> = (0..r.u8()).map(|_| (r.u8(), r.u8())).collect();
        let (t, c) = sets.get(set as usize).copied().unwrap_or((0, 0));
        let t = tables.into_iter().nth(usize::from(t));
        let t = t.unwrap_or(TableData {
            cats: 0,
            dict_start: 0,
            lookahead: 0,
            flags: 0,
            table: Vec::new(),
        });
        Self {
            cats: t.cats,
            dict_start: t.dict_start,
            lookahead: t.lookahead,
            bof: t.flags & 2 != 0,
            table: t.table,
            ranges: tries.into_iter().nth(usize::from(c)).unwrap_or_default(),
        }
    }

    /// Code point `c`'s category.
    fn category(&self, c: u32) -> u16 {
        let i = self.ranges.partition_point(|&(s, _)| s <= c);
        u16::from(i.checked_sub(1).map_or(0, |i| self.ranges[i].1))
    }

    /// `row[col]` of state `s`.
    fn at(&self, s: usize, col: usize) -> u16 {
        self.table
            .get(s * (self.cats + 2) + col)
            .copied()
            .unwrap_or(0)
    }

    /// The breaks ICU's line break iterator finds in `text` (UTF-16), as
    /// `ubrk_next` returns them from the start: each after the last, the
    /// last at `text.len()`. `None` if a stretch between two of them holds
    /// characters of the rules' dictionary classes (Thai, Lao, Khmer,
    /// Myanmar …), which ICU breaks further with its dictionaries.
    pub(crate) fn breaks(&self, text: &[u16]) -> Option<Vec<usize>> {
        let mut out = Vec::new();
        let mut look = alloc::vec![-1i64; self.lookahead.max(1)];
        let mut pos = 0;
        while pos < text.len() {
            let (next, dict) = self.handle_next(text, pos, &mut look);
            if dict {
                return None;
            }
            out.push(next);
            pos = next;
        }
        Some(out)
    }

    /// `handleNext`: the next break after `start` (`< text.len()`), and
    /// whether a dictionary character was met. `look` is the iterator's
    /// look-ahead positions, kept from run to run as ICU keeps them.
    fn handle_next(&self, text: &[u16], start: usize, look: &mut [i64]) -> (usize, bool) {
        #[derive(PartialEq)]
        enum Mode {
            Start,
            Run,
            End,
        }
        let mut dict = false;
        let mut idx = start;
        let mut result = start;
        let mut c = next32(text, &mut idx);
        let mut state = 1usize; // START_STATE
        let mut mode = Mode::Run;
        let mut category = 0u16;
        if self.bof {
            category = 2;
            mode = Mode::Start;
        }
        loop {
            if c.is_none() {
                if mode == Mode::End {
                    break;
                }
                mode = Mode::End;
                category = 1;
            }
            if mode == Mode::Run {
                category = self.category(c.unwrap_or(0));
                dict |= category >= self.dict_start;
            }
            state = usize::from(self.at(state, 2 + usize::from(category)));
            let accepting = self.at(state, 0);
            if accepting == 1 {
                if mode != Mode::Start {
                    result = idx;
                }
            } else if accepting > 1
                && let Some(&p) = look.get(usize::from(accepting))
                && let Ok(p) = usize::try_from(p)
            {
                // a look-ahead rule matched: the break where it recorded
                return (p, dict);
            }
            let rule = self.at(state, 1);
            if rule > 1
                && let Some(l) = look.get_mut(usize::from(rule))
            {
                *l = i64::try_from(idx).unwrap_or(-1);
            }
            if state == 0 {
                break;
            }
            if mode == Mode::Run {
                c = next32(text, &mut idx);
            } else if mode == Mode::Start {
                mode = Mode::Run;
            }
        }
        if result == start {
            // the rules matched nothing: one character on
            let mut i = start;
            next32(text, &mut i);
            result = i;
        }
        (result, dict)
    }
}

/// `UTEXT_NEXT32` over UTF-16: the code point at `*i` (a surrogate alone
/// as itself), `*i` moved past it; `None` at the end.
fn next32(text: &[u16], i: &mut usize) -> Option<u32> {
    let u = *text.get(*i)?;
    *i += 1;
    if (0xD800..0xDC00).contains(&u)
        && let Some(&t) = text.get(*i)
        && (0xDC00..0xE000).contains(&t)
    {
        *i += 1;
        return Some(0x10000 + ((u32::from(u) - 0xD800) << 10) + (u32::from(t) - 0xDC00));
    }
    Some(u32::from(u))
}

/// A rule set's classes from the first set's: from each start in `diffs`
/// on, the category given, or (0xFF) the first set's mapped by `map`.
fn overlay(first: &[(u32, u8)], map: &[u8], diffs: &[(u32, u8)]) -> Vec<(u32, u8)> {
    let mut points: Vec<u32> = first.iter().chain(diffs).map(|&(s, _)| s).collect();
    points.sort_unstable();
    points.dedup();
    let at = |r: &[(u32, u8)], c: u32| {
        let i = r.partition_point(|&(s, _)| s <= c);
        i.checked_sub(1).map_or(0, |i| r[i].1)
    };
    let mut out: Vec<(u32, u8)> = Vec::with_capacity(points.len());
    for p in points {
        let d = at(diffs, p);
        let v = if d == 0xFF {
            map.get(usize::from(at(first, p))).copied().unwrap_or(0)
        } else {
            d
        };
        if out.last().is_none_or(|&(_, l)| l != v) {
            out.push((p, v));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// Locales as ICU 78's `ubrk_open(UBRK_LINE, …)` takes them (the rule
    /// set it opens, by its compiled rules).
    #[test]
    fn locales() {
        use RuleSet::*;
        for (l, s) in [
            ("ja", Normal),
            ("ja_JP", Normal),
            ("ja-JP", Normal),
            ("JA", Normal),
            ("jpn", Normal),
            ("kor", Normal),
            ("zho", Cj),
            ("chi", Line),
            ("zh", Cj),
            ("zh_CN", Cj),
            ("zh-Hans", Cj),
            ("zh-Hant", Cj),
            ("zh_TW", Cj),
            ("zh_Hant_TW", Cj),
            ("ko", Normal),
            ("th", Line),
            ("kh", Line),
            ("G", Line),
            ("en", Line),
            ("de-1901", Line),
            ("ja@lb=strict", Cj),
            ("ja@lb=loose", LooseCj),
            ("ja@lb=normal", NormalCj),
            ("ja-u-lb-strict", Cj),
            ("ja-JP-u-lb-loose", LooseCj),
            ("ja@LB=loose", LooseCj),
            ("ja@lb=Loose", Normal),
            ("zh@lb=loose", LooseCj),
            ("zh@lb=normal", NormalCj),
            ("@lb=loose", Loose),
            ("en@lb=loose", Loose),
            ("en@lb=normal", Normal),
            ("en@lb=strict", Line),
            ("ko@lb=strict", Cj),
            ("ja@calendar=x;lb=loose", LooseCj),
            ("ja@ lb = loose", LooseCj),
            ("ja.utf8", Line),
            ("ja_JP.UTF-8", Normal),
            ("ja.UTF-8@lb=loose", Loose),
            ("zh@lw=phrase", Cj),
            ("ja-x-lb-loose", Normal),
            ("x-ja", Line),
            ("und-Jpan", Line),
            ("ja1", Line),
            ("zh-guoyu", Cj),
            ("ja_Latn_JP", Normal),
            ("zh-u-lb-normal-ca-x", NormalCj),
            ("ja@lb", Line),
            ("ja@lb=", Line),
            ("ja@lb=normal;lb=strict", NormalCj),
        ] {
            assert_eq!(rule_set(l.as_bytes()), Some(s), "{l}");
        }
        assert_eq!(rule_set(b"ja@lw=phrase"), None);
        assert_eq!(rule_set(b"ja@lb=loose;lw=phrase"), None);
    }

    /// Breaks as ICU 78 finds them.
    #[test]
    fn breaks() {
        let ja = Rules::new(RuleSet::Normal);
        let zh = Rules::new(RuleSet::Cj);
        let line = Rules::new(RuleSet::Line);
        let t = utf16("「こんにちは」、世界。");
        assert_eq!(ja.breaks(&t), Some(alloc::vec![2, 3, 4, 5, 8, 9, 11]));
        assert_eq!(zh.breaks(&t), Some(alloc::vec![2, 3, 4, 5, 8, 9, 11]));
        assert_eq!(line.breaks(&utf16("ab")), Some(alloc::vec![2]));
        assert_eq!(line.breaks(&utf16("ไทย")), None);
    }

    /// Against ICU4C itself: `PARTEX_LB_DIR` holds `cases.hex` (a text per
    /// line, its UTF-16 units in hex) and `exp_SET.txt` (ICU's breaks for
    /// each, from `ubrk_next`), as `target/x/icu` in the line break work
    /// made them.
    #[test]
    #[ignore = "needs ICU4C's answers"]
    fn against_icu4c() {
        extern crate std;
        use RuleSet::*;
        use std::string::String;
        let dir = std::env::var("PARTEX_LB_DIR").expect("PARTEX_LB_DIR");
        let cases = std::fs::read_to_string(std::format!("{dir}/cases.hex")).unwrap();
        let texts: Vec<Vec<u16>> = cases
            .lines()
            .map(|l| {
                l.split_whitespace()
                    .map(|h| u16::from_str_radix(h, 16).unwrap())
                    .collect()
            })
            .collect();
        let mut bad = 0;
        for (name, set) in [
            ("Line", Line),
            ("Loose", Loose),
            ("Normal", Normal),
            ("Cj", Cj),
            ("LooseCj", LooseCj),
            ("NormalCj", NormalCj),
        ] {
            let rules = Rules::new(set);
            let exp = std::fs::read_to_string(std::format!("{dir}/exp_{name}.txt")).unwrap();
            let (mut same, mut differ, mut dict, mut dict_same) = (0, 0, 0, 0);
            for (t, e) in texts.iter().zip(exp.lines()) {
                let e: Vec<usize> = e.split_whitespace().map(|x| x.parse().unwrap()).collect();
                match rules.breaks(t) {
                    Some(b) if b == e => same += 1,
                    Some(b) => {
                        differ += 1;
                        if differ <= 5 {
                            let s: String = char::decode_utf16(t.iter().copied())
                                .map(|c| c.unwrap_or('\u{FFFD}'))
                                .collect();
                            std::eprintln!("{name}: {t:x?} {s:?}: icu {e:?}, ours {b:?}");
                        }
                    }
                    None => {
                        dict += 1;
                        // what the rules alone give, to see what the
                        // dictionaries change
                        let mut look = alloc::vec![-1i64; rules.lookahead.max(1)];
                        let (mut pos, mut b) = (0, Vec::new());
                        while pos < t.len() {
                            pos = rules.handle_next(t, pos, &mut look).0;
                            b.push(pos);
                        }
                        dict_same += usize::from(b == e);
                    }
                }
            }
            std::eprintln!(
                "{name}: {} texts: {same} same, {differ} differ; {dict} with dictionary characters (rules alone right on {dict_same})",
                texts.len()
            );
            bad += differ;
        }
        assert_eq!(bad, 0);
    }
}
