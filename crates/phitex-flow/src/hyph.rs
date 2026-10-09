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
//! dictionary; [`hyphenate_after`] (§894–918) is the line breaker's step:
//! which word after a glue is tried, and how the found positions become
//! discretionaries with the font's ligatures and kerns. That last step can
//! drop a position: §916 builds a discretionary's post-break text with
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

// ------------------------------------------------------------ a paragraph's words

/// What hyphenating a paragraph's words reads (§891–§899), read where the
/// paragraph is broken: the language and minima it began with (§1091),
/// `\uchyph`, the `\lccode`s of its characters, its fonts' `\hyphenchar`s,
/// and the tables of each language it uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HyphParams {
    /// `init_cur_lang`, `init_l_hyf`, `init_r_hyf` (§891).
    pub init: LangState,
    pub uc_hyph: bool,
    /// The `\lccode` of each character the paragraph has (0 for the rest:
    /// never consulted).
    pub lc: Vec<u8>,
    /// Each font's `\hyphenchar`, by `Font::key`.
    pub hyphen_chars: Vec<(Arc<str>, i32)>,
    /// Each language's patterns and exceptions.
    pub tables: Vec<(u8, Patterns, Exceptions)>,
    /// `\tracinglostchars > 0` (a missing hyphen character is reported).
    pub lost_chars: bool,
}

/// `cur_lang`, `l_hyf`, `r_hyf` (§892): the language whatsits change them
/// as the line breaker passes them (§1362–§1363).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct LangState {
    pub lang: u8,
    pub lhm: u8,
    pub rhm: u8,
}

/// A word hyphenated: `items[start..end]` becomes `items`, with the
/// discretionaries; `missing`: the hyphen character the font lacks, each
/// time `new_character` looked for it (§915).
#[derive(Clone, Debug)]
pub struct Hyphenated {
    pub start: usize,
    pub end: usize,
    pub items: Vec<Item>,
    pub missing: Vec<(Arc<Font>, i32)>,
}

use crate::item::{Disc, Item, Lig};
use crate::tfm::{Font, LigKern, Tag};

/// tex.web 549: `non_char`.
const NON_CHAR: i32 = 256;

/// §894–§903, §913–§918: the word after the glue `items[cur_p]`,
/// hyphenated, if TeX would (`st`: the language state, changed by the
/// language whatsits passed on the way, as §1363 does).
pub fn hyphenate_after(items: &[Item], cur_p: usize, st: &mut LangState, p: &HyphParams) -> Option<Hyphenated> {
    let lc = |c: u8| p.lc.get(usize::from(c)).copied().unwrap_or(0);
    // (§896: skip to node `ha`)
    let mut prev_s = cur_p;
    let mut s = cur_p + 1;
    let hf: Arc<Font> = loop {
        let (c, f) = match items.get(s)? {
            Item::Char(f, c) => (*c, f.clone()),
            Item::Lig(l) => {
                let Some(&c) = l.orig.first() else {
                    prev_s = s;
                    s += 1;
                    continue;
                };
                (c, l.font.clone())
            }
            // (§896: and any other whatsit is passed)
            Item::Kern { sub: 0, .. } | Item::Whatsit(_) => {
                prev_s = s;
                s += 1;
                continue;
            }
            Item::Lang { lang, lhm, rhm } => {
                *st = LangState { lang: *lang, lhm: *lhm, rhm: *rhm };
                prev_s = s;
                s += 1;
                continue;
            }
            _ => return None,
        };
        if lc(c) != 0 {
            if lc(c) == c || p.uc_hyph {
                break f;
            }
            return None;
        }
        prev_s = s;
        s += 1;
    };
    let hyf_char = p.hyphen_chars.iter().find(|(k, _)| **k == *hf.key).map_or(-1, |(_, h)| *h);
    if !(0..=255).contains(&hyf_char) {
        return None;
    }
    let ha = prev_s;
    let (l_hyf, r_hyf) = (usize::from(st.lhm), usize::from(st.rhm));
    if l_hyf + r_hyf > 63 {
        return None;
    }
    // (§897: skip to node `hb`, the letters into `hu` and `hc`)
    let font_bchar = hf.bchar;
    let mut hu = [0i32; 66];
    let mut hn = 0usize;
    let mut hb = s;
    let mut hyf_bchar = NON_CHAR;
    loop {
        match items.get(s) {
            Some(Item::Char(f, c)) => {
                if **f != *hf {
                    break;
                }
                hyf_bchar = i32::from(*c);
                if lc(*c) == 0 || hn == 63 {
                    break;
                }
                hb = s;
                hn += 1;
                hu[hn] = i32::from(*c);
                hyf_bchar = NON_CHAR;
            }
            Some(Item::Lig(l)) => {
                // (§898)
                if *l.font != *hf {
                    break;
                }
                let mut j = hn;
                if let Some(&q) = l.orig.first() {
                    hyf_bchar = i32::from(q);
                }
                let mut letters = true;
                for &c in &l.orig {
                    if lc(c) == 0 || j == 63 {
                        letters = false;
                        break;
                    }
                    j += 1;
                    hu[j] = i32::from(c);
                }
                if !letters {
                    break;
                }
                hb = s;
                hn = j;
                hyf_bchar = if l.right { font_bchar } else { NON_CHAR };
            }
            Some(Item::Kern { sub: 0, .. }) => {
                hb = s;
                hyf_bchar = font_bchar;
            }
            _ => break,
        }
        s += 1;
    }
    // (§899: what follows the word permits hyphenation)
    if hn < l_hyf + r_hyf {
        return None;
    }
    loop {
        match items.get(s)? {
            Item::Char(..) | Item::Lig(_) => {}
            Item::Kern { sub, .. } => {
                if *sub != 0 {
                    break;
                }
            }
            Item::Lang { .. }
            | Item::Glue { .. }
            | Item::Leaders(_)
            | Item::Penalty(_)
            | Item::Mark(_)
            | Item::Marks(..)
            | Item::Whatsit(_)
            | Item::Ins(_)
            | Item::Adjust(_) => break,
            _ => return None,
        }
        s += 1;
    }
    // (§923: the hyphens)
    let (pats, excs) = p
        .tables
        .iter()
        .find(|(l, ..)| *l == st.lang)
        .map_or_else(|| (Patterns::new(), Exceptions::new()), |(_, a, b)| (a.clone(), b.clone()));
    let word: Vec<char> = (1..=hn).map(|j| char::from(hu[j] as u8)).collect();
    let lcc = |c: char| char::from(lc(c as u8));
    let at = hyphenate(&word, lcc, i32::from(st.lhm), i32::from(st.rhm), &pats, &excs);
    if at.is_empty() {
        return None;
    }
    let mut hyf = [0u8; 66];
    for j in at {
        hyf[j] = 1;
    }
    // (§903: the nodes `ha..hb` replaced)
    let mut w = Word {
        font: hf.clone(),
        hu,
        hyf,
        hyf_char,
        font_bchar,
        init_list: Vec::new(),
        init_lig: false,
        init_lft: false,
        hyphen_passed: 0,
        missing: Vec::new(),
    };
    let found2 = |w: &mut Word| {
        w.hu[0] = NON_CHAR;
        w.init_lig = false;
        w.init_list.clear();
    };
    let (start, j) = match &items[ha] {
        Item::Char(f, c) if **f == *hf => {
            w.init_list = vec![*c];
            w.init_lig = false;
            w.hu[0] = i32::from(*c);
            (ha, 0)
        }
        Item::Lig(l) if *l.font == *hf => {
            w.init_list.clone_from(&l.orig);
            w.init_lig = true;
            w.init_lft = l.left;
            w.hu[0] = i32::from(l.c);
            if w.init_list.is_empty() && w.init_lft {
                w.hu[0] = NON_CHAR;
                w.init_lig = false;
            }
            (ha, 0)
        }
        Item::Char(..) | Item::Lig(_) => {
            found2(&mut w);
            (ha + 1, 0)
        }
        _ => {
            if let Some(Item::Lig(l)) = items.get(ha + 1)
                && l.left
            {
                found2(&mut w);
                (ha + 1, 0)
            } else {
                (ha + 1, 1)
            }
        }
    };
    let out = w.reconstitute_word(j, hn, hyf_bchar);
    Some(Hyphenated { start, end: hb + 1, items: out, missing: w.missing })
}

/// §900, §905: the word being reconstituted.
struct Word {
    font: Arc<Font>,
    hu: [i32; 66],
    hyf: [u8; 66],
    hyf_char: i32,
    font_bchar: i32,
    init_list: Vec<u8>,
    init_lig: bool,
    init_lft: bool,
    hyphen_passed: usize,
    missing: Vec<(Arc<Font>, i32)>,
}

/// §907: the ligature cursor of `reconstitute`.
struct Cursor {
    cur_l: i32,
    cur_r: i32,
    /// `t`'s length at `cur_q`: the characters after it form the next
    /// ligature.
    cur_q: usize,
    /// The stack of inserted ligature characters (top last), each with the
    /// character it holds for `hu[j+1]`, if any.
    lig_stack: Vec<(i32, Option<u8>)>,
    ligature_present: bool,
    lft_hit: bool,
    rt_hit: bool,
}

impl Word {
    fn ch(&self, c: i32) -> Item {
        Item::Char(self.font.clone(), c as u8)
    }

    /// §913: the word's nodes, from `hu[j]`, with discretionary hyphens.
    fn reconstitute_word(&mut self, mut j: usize, hn: usize, bchar: i32) -> Vec<Item> {
        let mut out: Vec<Item> = Vec::new();
        loop {
            let mut l = j;
            let (next, mut hold) = self.reconstitute(j, hn, bchar, self.hyf_char);
            j = next + 1;
            if self.hyphen_passed == 0 {
                out.append(&mut hold);
                if self.hyf[j - 1] % 2 == 1 {
                    l = j;
                    self.hyphen_passed = j - 1;
                }
            }
            if self.hyphen_passed > 0 {
                self.discretionaries(&mut out, hold, &mut j, &mut l, hn, bchar);
            }
            if j > hn {
                break;
            }
        }
        out
    }

    /// §914–§918: a discretionary as an alternative to the unhyphenated
    /// word, both branches developed until they are the same again.
    fn discretionaries(
        &mut self,
        out: &mut Vec<Item>,
        start: Vec<Item>,
        j: &mut usize,
        l: &mut usize,
        hn: usize,
        bchar: i32,
    ) {
        let mut major = start;
        loop {
            let mut i = self.hyphen_passed;
            self.hyf[i] = 0;
            // (§915: `hu[l..i]` and a hyphen, the pre-break text)
            let mut pre = Vec::new();
            let exists = self.font.glyph(self.hyf_char).is_some();
            let mut c = 0;
            if exists {
                i += 1;
                c = self.hu[i];
                self.hu[i] = self.hyf_char;
            } else {
                self.missing.push((self.font.clone(), self.hyf_char));
            }
            while *l <= i {
                let (next, hold) = self.reconstitute(*l, i, self.font_bchar, NON_CHAR);
                *l = next + 1;
                pre.extend(hold);
            }
            if exists {
                self.hu[i] = c;
                *l = i;
            }
            // (§916: `hu[i+1..]`, the post-break text, until it is in step
            // with the unhyphenated one)
            let mut post = Vec::new();
            let mut c_loc = 0;
            if self.font.bchar_label.is_some() {
                *l -= 1;
                c = self.hu[*l];
                c_loc = *l;
                self.hu[*l] = NON_CHAR;
            }
            while *l < *j {
                loop {
                    let (next, hold) = self.reconstitute(*l, hn, bchar, NON_CHAR);
                    *l = next + 1;
                    if c_loc > 0 {
                        self.hu[c_loc] = c;
                        c_loc = 0;
                    }
                    post.extend(hold);
                    if *l >= *j {
                        break;
                    }
                }
                while *l > *j {
                    // (§917)
                    let (next, hold) = self.reconstitute(*j, hn, bchar, NON_CHAR);
                    *j = next + 1;
                    major.extend(hold);
                }
            }
            // (§918)
            let taken = std::mem::take(&mut major);
            if taken.len() <= 127 {
                out.push(Item::Disc(Arc::new(Disc { pre, post, replace: taken.len() as u8 })));
            }
            out.extend(taken);
            self.hyphen_passed = *j - 1;
            if self.hyf[*j - 1].is_multiple_of(2) {
                return;
            }
        }
    }

    /// §908: `set_cur_r`; returns `cur_rh`.
    fn set_cur_r(&self, cur: &mut Cursor, j: usize, n: usize, bchar: i32, hchar: i32) -> i32 {
        cur.cur_r = if j < n { self.hu[j + 1] } else { bchar };
        if self.hyf[j] % 2 == 1 { hchar } else { NON_CHAR }
    }

    /// §910: `wrap_lig(b)`.
    fn wrap_lig(&self, cur: &mut Cursor, t: &mut Vec<Item>, b: bool) {
        if !cur.ligature_present {
            return;
        }
        let orig: Vec<u8> =
            t.drain(cur.cur_q..).filter_map(|i| if let Item::Char(_, c) = i { Some(c) } else { None }).collect();
        let mut left = false;
        if cur.lft_hit {
            left = true;
            cur.lft_hit = false;
        }
        let mut right = false;
        if b && cur.lig_stack.is_empty() {
            right = true;
            cur.rt_hit = false;
        }
        t.push(Item::Lig(Arc::new(Lig { font: self.font.clone(), c: cur.cur_l as u8, orig, left, right })));
        cur.ligature_present = false;
    }

    /// §910: `pop_lig_stack`; returns the new `cur_rh`.
    #[allow(clippy::too_many_arguments)]
    fn pop_lig_stack(
        &self,
        cur: &mut Cursor,
        t: &mut Vec<Item>,
        j: &mut usize,
        n: usize,
        bchar: i32,
        hchar: i32,
        cur_rh: i32,
    ) -> i32 {
        let (_, held) = cur.lig_stack.pop().unwrap_or((0, None));
        if let Some(c) = held {
            t.push(self.ch(i32::from(c)));
            *j += 1;
        }
        if let Some(&(top, _)) = cur.lig_stack.last() {
            cur.cur_r = top;
            cur_rh
        } else {
            self.set_cur_r(cur, *j, n, bchar, hchar)
        }
    }

    /// §905–§911: `reconstitute(j, n, bchar, hchar)`: the nodes for
    /// `hu[j..]` as far as one ligature/kern step goes; the index of the
    /// last character used, and the nodes.
    fn reconstitute(&mut self, mut j: usize, n: usize, mut bchar: i32, mut hchar: i32) -> (usize, Vec<Item>) {
        self.hyphen_passed = 0;
        let mut t: Vec<Item> = Vec::new();
        let mut w = 0;
        let font = self.font.clone();
        // (§908)
        let mut cur = Cursor {
            cur_l: self.hu[j],
            cur_r: 0,
            cur_q: 0,
            lig_stack: Vec::new(),
            ligature_present: false,
            lft_hit: false,
            rt_hit: false,
        };
        if j == 0 {
            cur.ligature_present = self.init_lig;
            if cur.ligature_present {
                cur.lft_hit = self.init_lft;
            }
            for &c in &self.init_list {
                t.push(self.ch(i32::from(c)));
            }
        } else if cur.cur_l < NON_CHAR {
            t.push(self.ch(cur.cur_l));
        }
        let mut cur_rh = self.set_cur_r(&mut cur, j, n, bchar, hchar);
        'continue_: loop {
            // (§909)
            'done: {
                let mut k = if cur.cur_l == NON_CHAR {
                    match font.bchar_label {
                        Some(k) => usize::from(k),
                        None => break 'done,
                    }
                } else {
                    let Some(g) = font.glyph(cur.cur_l) else { break 'done };
                    let Tag::Lig(start) = g.tag else { break 'done };
                    let mut k = usize::from(start);
                    let q = font.lig_kerns[k];
                    if q.skip > LigKern::STOP {
                        k = 256 * usize::from(q.op) + usize::from(q.remainder);
                    }
                    k
                };
                let test_char = if cur_rh < NON_CHAR { cur_rh } else { cur.cur_r };
                loop {
                    let q = font.lig_kerns[k];
                    if i32::from(q.next) == test_char && q.skip <= LigKern::STOP {
                        if cur_rh < NON_CHAR {
                            self.hyphen_passed = j;
                            hchar = NON_CHAR;
                            cur_rh = NON_CHAR;
                            continue 'continue_;
                        }
                        if hchar < NON_CHAR && self.hyf[j] % 2 == 1 {
                            self.hyphen_passed = j;
                            hchar = NON_CHAR;
                        }
                        if q.op < LigKern::KERN {
                            // (§911)
                            if cur.cur_l == NON_CHAR {
                                cur.lft_hit = true;
                            }
                            if j == n && cur.lig_stack.is_empty() {
                                cur.rt_hit = true;
                            }
                            let rem = i32::from(q.remainder);
                            match q.op {
                                1 | 5 => {
                                    cur.cur_l = rem;
                                    cur.ligature_present = true;
                                }
                                2 | 6 => {
                                    cur.cur_r = rem;
                                    if let Some(top) = cur.lig_stack.last_mut() {
                                        top.0 = rem;
                                    } else if j == n {
                                        cur.lig_stack.push((rem, None));
                                        bchar = NON_CHAR;
                                    } else {
                                        cur.lig_stack.push((rem, Some(self.hu[j + 1] as u8)));
                                    }
                                }
                                3 => {
                                    cur.cur_r = rem;
                                    cur.lig_stack.push((rem, None));
                                }
                                7 | 11 => {
                                    self.wrap_lig(&mut cur, &mut t, false);
                                    cur.cur_q = t.len();
                                    cur.cur_l = rem;
                                    cur.ligature_present = true;
                                }
                                _ => {
                                    cur.cur_l = rem;
                                    cur.ligature_present = true;
                                    if !cur.lig_stack.is_empty() {
                                        cur_rh = self.pop_lig_stack(&mut cur, &mut t, &mut j, n, bchar, hchar, cur_rh);
                                    } else if j == n {
                                        break 'done;
                                    } else {
                                        t.push(self.ch(cur.cur_r));
                                        j += 1;
                                        cur_rh = self.set_cur_r(&mut cur, j, n, bchar, hchar);
                                    }
                                }
                            }
                            if q.op > 4 && q.op != 7 {
                                break 'done;
                            }
                            continue 'continue_;
                        }
                        let ki = 256 * usize::from(q.op - LigKern::KERN) + usize::from(q.remainder);
                        w = font.kerns.get(ki).copied().unwrap_or(0);
                        break 'done;
                    }
                    if q.skip >= LigKern::STOP {
                        if cur_rh == NON_CHAR {
                            break 'done;
                        }
                        cur_rh = NON_CHAR;
                        continue 'continue_;
                    }
                    k += usize::from(q.skip) + 1;
                }
            }
            // (§910)
            let rt = cur.rt_hit;
            self.wrap_lig(&mut cur, &mut t, rt);
            if w != 0 {
                t.push(Item::Kern { w, sub: 0 });
                w = 0;
            }
            if let Some(&(top, _)) = cur.lig_stack.last() {
                cur.cur_q = t.len();
                cur.cur_l = top;
                cur.ligature_present = true;
                cur_rh = self.pop_lig_stack(&mut cur, &mut t, &mut j, n, bchar, hchar, cur_rh);
                continue 'continue_;
            }
            break;
        }
        (j, t)
    }
}
