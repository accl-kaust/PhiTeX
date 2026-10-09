//! Hyphenation as pure functions over values: tex.web parts 40–43
//! (§891–965).
//!
//! [`Patterns`] is one language's `\patterns` (§960–965) and
//! [`Exceptions`] its `\hyphenation` list (§934–941): immutable values,
//! built by pure functions from what the commands read, shared by `Arc`.
//! A later `\patterns` or `\hyphenation` for the same language makes a new
//! value from the old one ([`Patterns::add`], [`Exceptions::add`]); on the
//! graph they are the definitions of `Key::Patterns(language)` and
//! `Key::Exceptions(language)` (CORE.md, "Hyphenation").
//!
//! tex.web packs the patterns into a trie (§920–958); a map from each
//! pattern's letters to its digits gives the same answers: the trie walk of
//! §923 visits exactly the patterns whose letters occur in the word, and
//! §924 takes, at each position, the largest digit any of them puts there.
//! [`hyphenate`] (§923–932) does that for one word, after the exception
//! dictionary; which words are tried, and how the found positions become
//! discretionaries (§894–918), belong to the line breaker. That last step
//! can drop a position: §916 builds a discretionary's post-break text with
//! no hyphen character, so a break inside it is lost (`raffish` with both
//! minima 1, in cmr10: [`hyphenate`] gives `raf-f-ish`, TeX shows
//! `raf-fish`, the second break falling in the `ffi` ligature's post-break
//! text).

use std::collections::BTreeMap;
use std::sync::Arc;

/// What `\patterns` and `\hyphenation` read inside their braces, the
/// expansion done: a character (a letter or other character; for
/// `\hyphenation` also a `\chardef`'d token or `\char`), a space (a
/// spacer, which ends a pattern or word), or anything else (an error).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tok {
    Char(char),
    Space,
    Bad,
}

/// The error heads `\patterns` and `\hyphenation` give (each one TeX's
/// `print_err`, its help Appendix H).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HyphError {
    /// §962: a pattern's character with `\lccode` 0 (it is kept, as 0).
    Nonletter,
    /// §963: a pattern whose letters an earlier one (with digits) had; the
    /// new digits replace the old.
    DuplicatePattern,
    /// §961: a token that is not a character, space or `}`.
    BadPatterns,
    /// §937: a `\hyphenation` character with `\lccode` 0 (it is dropped).
    NotALetter,
    /// §936: a token that is not a character, space or `}`.
    ImproperHyphenation,
}

impl HyphError {
    /// The message as TeX prints it after `! `.
    pub fn message(&self) -> &'static str {
        match self {
            HyphError::Nonletter => "Nonletter",
            HyphError::DuplicatePattern => "Duplicate pattern",
            HyphError::BadPatterns => "Bad \\patterns",
            HyphError::NotALetter => "Not a letter",
            HyphError::ImproperHyphenation => "Improper \\hyphenation will be flushed",
        }
    }
}

/// §960 `set_cur_lang`: the language `\patterns`, `\hyphenation` and a
/// paragraph's words use, from `\language`.
pub fn cur_lang(language: i32) -> u8 {
    if language <= 0 || language > 255 { 0 } else { language as u8 }
}

/// §1091 `norm_min`: `\lefthyphenmin` and `\righthyphenmin` as a
/// paragraph keeps them, 1 to 63.
pub fn norm_min(h: i32) -> usize {
    h.clamp(1, 63) as usize
}

/// §891: a font's `\hyphenchar` outside 0–255 hyphenates no word of it.
pub fn hyphen_char_allows(hyphen_char: i32) -> bool {
    (0..=255).contains(&hyphen_char)
}

/// §896: a word whose first letter is not its own lowercase is hyphenated
/// only if `\uchyph` is positive.
pub fn uc_hyph_allows(first: char, lc: impl Fn(char) -> char, uc_hyph: i32) -> bool {
    lc(first) == first || uc_hyph > 0
}

/// The longest pattern or exception word (`k<63`, `n<63`).
const MAX: usize = 63;

/// One language's hyphenation patterns: each pattern's letters (lc codes;
/// `.`, the word's edge, as 0) to its digits, `digits[d]` the one after the
/// pattern's `d`-th letter (§962).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Patterns(Arc<PatternMap>);

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
struct PatternMap {
    map: BTreeMap<Box<[char]>, Box<[u8]>>,
    /// The most letters a pattern has (bounds the walk of §923).
    longest: usize,
}

impl Patterns {
    /// No pattern: §923's "no patterns for `cur_lang`".
    pub fn new() -> Patterns {
        Patterns::default()
    }

    pub fn is_empty(&self) -> bool {
        self.0.map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.map.len()
    }

    /// `\patterns{...}` (§961–965): these patterns and the ones `toks`
    /// gives, with the errors TeX reports, in order.
    pub fn add(&self, toks: &[Tok], lc: impl Fn(char) -> char) -> (Patterns, Vec<HyphError>) {
        let mut m = (*self.0).clone();
        let mut errors = Vec::new();
        let mut hc = [0 as char; MAX + 1];
        let mut hyf = [0u8; MAX + 1];
        let (mut k, mut digit_sensed) = (0usize, false);
        for t in toks.iter().copied().chain([Tok::Space]) {
            match t {
                Tok::Char(c) => {
                    if digit_sensed || !c.is_ascii_digit() {
                        let c = if c == '.' {
                            '\0'
                        } else {
                            let l = lc(c);
                            if l == '\0' {
                                errors.push(HyphError::Nonletter);
                            }
                            l
                        };
                        if k < MAX {
                            k += 1;
                            hc[k] = c;
                            hyf[k] = 0;
                            digit_sensed = false;
                        }
                    } else if k < MAX {
                        hyf[k] = c as u8 - b'0';
                        digit_sensed = true;
                    }
                }
                Tok::Space => {
                    if k > 0 {
                        // §965: no break at the word's edges.
                        if hc[1] == '\0' {
                            hyf[0] = 0;
                        }
                        if hc[k] == '\0' {
                            hyf[k] = 0;
                        }
                        let digits: Box<[u8]> = hyf[..=k].into();
                        // §963: an earlier pattern with an op is a duplicate.
                        let prev = m.map.insert(hc[1..=k].into(), digits);
                        if prev.is_some_and(|p| p.iter().any(|&d| d != 0)) {
                            errors.push(HyphError::DuplicatePattern);
                        }
                        m.longest = m.longest.max(k);
                    }
                    k = 0;
                    hyf[0] = 0;
                    digit_sensed = false;
                }
                Tok::Bad => errors.push(HyphError::BadPatterns),
            }
        }
        (Patterns(Arc::new(m)), errors)
    }

    /// The digits of the pattern with these letters (`.` as 0), if any.
    pub fn get(&self, letters: &[char]) -> Option<&[u8]> {
        self.0.map.get(letters).map(|d| &d[..])
    }
}

/// One language's `\hyphenation` exceptions: each word's lc-coded letters
/// to the number of letters before each of its hyphens (§934–940).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Exceptions(Arc<ExceptionMap>);

type ExceptionMap = BTreeMap<Box<[char]>, Box<[usize]>>;

impl Exceptions {
    pub fn new() -> Exceptions {
        Exceptions::default()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `\hyphenation{...}` (§935–940): these exceptions and the words
    /// `toks` gives, with the errors TeX reports. A word given again
    /// replaces the earlier one (§941 puts the newer first in its probe
    /// sequence, where §931 finds it).
    pub fn add(&self, toks: &[Tok], lc: impl Fn(char) -> char) -> (Exceptions, Vec<HyphError>) {
        let mut m = (*self.0).clone();
        let mut errors = Vec::new();
        let mut word: Vec<char> = Vec::new();
        let mut hyphens: Vec<usize> = Vec::new();
        for t in toks.iter().copied().chain([Tok::Space]) {
            match t {
                Tok::Char('-') => {
                    if word.len() < MAX {
                        hyphens.push(word.len());
                    }
                }
                Tok::Char(c) => {
                    let l = lc(c);
                    if l == '\0' {
                        errors.push(HyphError::NotALetter);
                    } else if word.len() < MAX {
                        word.push(l);
                    }
                }
                Tok::Space => {
                    if word.len() > 1 {
                        hyphens.sort_unstable();
                        hyphens.dedup();
                        m.insert(std::mem::take(&mut word).into(), std::mem::take(&mut hyphens).into());
                    }
                    word.clear();
                    hyphens.clear();
                }
                Tok::Bad => errors.push(HyphError::ImproperHyphenation),
            }
        }
        (Exceptions(Arc::new(m)), errors)
    }

    /// The hyphens of the word with these lc-coded letters, if it is one.
    pub fn get(&self, letters: &[char]) -> Option<&[usize]> {
        self.0.get(letters).map(|h| &h[..])
    }
}

/// §923–932: where the word may be hyphenated — each `j` a break after
/// its `j`-th letter — by its exception if it has one, else by the
/// patterns; never within `lhmin` letters of its start or `rhmin` of its
/// end (each made 1–63 by [`norm_min`]), and nowhere in a word shorter
/// than `lhmin + rhmin` (§899).
///
/// `word` is the letters §897 collected (at most 63, all of one font and
/// with nonzero `\lccode`), `lc` their `\lccode`s.
pub fn hyphenate(
    word: &[char],
    lc: impl Fn(char) -> char,
    lhmin: i32,
    rhmin: i32,
    patterns: &Patterns,
    exceptions: &Exceptions,
) -> Vec<usize> {
    let (l_hyf, r_hyf) = (norm_min(lhmin), norm_min(rhmin));
    let hn = word.len();
    if hn < l_hyf + r_hyf {
        return Vec::new();
    }
    let mut hc: Vec<char> = Vec::with_capacity(hn + 2);
    hc.push('\0');
    hc.extend(word.iter().map(|&c| lc(c)));
    hc.push('\0');
    let mut hyf = vec![0u8; hn + 2];
    if let Some(hyphens) = exceptions.get(&hc[1..=hn]) {
        // §932
        for &h in hyphens {
            hyf[h] = 1;
        }
    } else if !patterns.is_empty() {
        // §923–924: every pattern starting at `j` (0, the edge, to
        // `hn - r_hyf + 1`), its digits at the maximum.
        let p = &patterns.0;
        for j in 0..=hn + 1 - r_hyf {
            for l in j..(j + p.longest).min(hn + 2) {
                if let Some(d) = p.map.get(&hc[j..=l]) {
                    for (i, &v) in d.iter().enumerate() {
                        // the digit after the pattern's i-th letter: after
                        // the word's letter j + i - 1 (none before hc[0])
                        if let Some(at) = (j + i).checked_sub(1) {
                            hyf[at] = hyf[at].max(v);
                        }
                    }
                }
            }
        }
    }
    // §923 `found`, §902: the odd values between the minima.
    (l_hyf..=hn - r_hyf).filter(|&j| hyf[j] % 2 == 1).collect()
}

/// The `\patterns{...}` and `\hyphenation{...}` blocks of a file like
/// `hyphen.tex`, read without a tokenizer: `%` comments and line ends
/// dropped (a line end is a space), each block's text as [`Tok`]s. Every
/// character in a block is a [`Tok::Char`], every run of white space a
/// [`Tok::Space`].
pub fn read_blocks(text: &str) -> Vec<(Block, Vec<Tok>)> {
    let mut clean = String::with_capacity(text.len());
    for line in text.lines() {
        clean.push_str(line.split('%').next().unwrap_or(""));
        clean.push(' ');
    }
    let mut out = Vec::new();
    let mut rest = clean.as_str();
    while let Some(at) = rest.find('\\') {
        rest = &rest[at + 1..];
        let block = if let Some(r) = rest.strip_prefix("patterns") {
            rest = r;
            Block::Patterns
        } else if let Some(r) = rest.strip_prefix("hyphenation") {
            rest = r;
            Block::Hyphenation
        } else {
            continue;
        };
        let Some(open) = rest.find('{') else { break };
        let body = &rest[open + 1..];
        let close = body.find('}').unwrap_or(body.len());
        let mut toks = Vec::new();
        for c in body[..close].chars() {
            if c.is_whitespace() {
                if toks.last() != Some(&Tok::Space) {
                    toks.push(Tok::Space);
                }
            } else {
                toks.push(Tok::Char(c));
            }
        }
        out.push((block, toks));
        rest = &body[(close + 1).min(body.len())..];
    }
    out
}

/// Which command a block of [`read_blocks`] is the argument of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Block {
    Patterns,
    Hyphenation,
}
