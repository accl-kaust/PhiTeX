//! Hyphenation (§891–§965): Liang's patterns and the exception
//! dictionary as plain data, the search for a hyphenatable word after a
//! glue node, and the reconstitution of its ligatures and kerns around
//! the discretionary hyphens (§903–§918).
//!
//! e-TeX's `\savinghyphcodes` is supported through
//! [`Patterns::set_hyph_codes`]; `TeXXeT`'s math nodes are out of scope.

use std::collections::{BTreeMap, HashMap};

use crate::fonts::{FontId, Fonts, Left, LigKern, LigOp};
use crate::linebreak::{Hyphenator, LangState};
use crate::node::{Disc, KernKind, Ligature, List, Node};

/// `non_char` (§549): no character, or the left boundary in `hu`.
pub const NON_CHAR: u16 = 256;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TrieNode {
    children: BTreeMap<u16, TrieNode>,
    /// `(position, value)` pairs, `n_i` nonzero only (§965).
    ops: Vec<(usize, u8)>,
}

/// What `\patterns` would complain about (§961–§963).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PatternReport {
    /// "Nonletter": a character with `\lccode` 0 (entered as 0 anyway).
    pub nonletter: bool,
    /// "Duplicate pattern": the new values replace the old.
    pub duplicate: bool,
}

/// Hyphenation patterns and exceptions for every language.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Patterns {
    tries: HashMap<u16, TrieNode>,
    exceptions: HashMap<(u16, Vec<u16>), Vec<usize>>,
    hyph_codes: HashMap<u16, Vec<u16>>,
    /// The tries and exceptions laid flat for the search (made on the
    /// first word, dropped by every change).
    flat: FlatCache,
}

/// The tries packed into arrays (tex.web's `trie`, §920–§921, as a
/// node table: each node's children are a run of keys and node numbers,
/// its ops a run of `(position, value)`), and the exceptions sorted by
/// a hash of the language and word.
#[derive(Debug, Default)]
struct Flat {
    roots: Vec<(u16, u32)>,
    /// Per node: first child, child count, first op, op count.
    nodes: Vec<[u32; 4]>,
    keys: Vec<u16>,
    kids: Vec<u32>,
    ops: Vec<(u8, u8)>,
    exceptions: Vec<(u64, u16, Vec<u16>, Vec<usize>)>,
}

/// A derived value: no part of what the patterns are (equal, cloned
/// empty).
#[derive(Default)]
struct FlatCache(std::sync::OnceLock<Flat>);

impl Clone for FlatCache {
    fn clone(&self) -> Self {
        FlatCache::default()
    }
}

impl PartialEq for FlatCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for FlatCache {}

impl std::fmt::Debug for FlatCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FlatCache")
    }
}

fn word_hash(lang: u16, word: &[u16]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ u64::from(lang);
    for &c in word {
        h ^= u64::from(c);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

impl Flat {
    fn of(p: &Patterns) -> Flat {
        fn put(f: &mut Flat, n: &TrieNode) -> u32 {
            let me = u32::try_from(f.nodes.len()).unwrap_or(u32::MAX);
            f.nodes.push([0; 4]);
            let ops0 = u32::try_from(f.ops.len()).unwrap_or(u32::MAX);
            f.ops.extend(
                n.ops
                    .iter()
                    .map(|&(i, v)| (u8::try_from(i).unwrap_or(u8::MAX), v)),
            );
            // (the children's runs are contiguous: keys first, numbers
            // filled in as each child is laid out)
            let k0 = f.keys.len();
            f.keys.extend(n.children.keys().copied());
            f.kids.extend(std::iter::repeat_n(0, n.children.len()));
            for (j, c) in n.children.values().enumerate() {
                let x = put(f, c);
                f.kids[k0 + j] = x;
            }
            f.nodes[me as usize] = [
                u32::try_from(k0).unwrap_or(u32::MAX),
                u32::try_from(n.children.len()).unwrap_or(u32::MAX),
                ops0,
                u32::try_from(n.ops.len()).unwrap_or(u32::MAX),
            ];
            me
        }
        let mut f = Flat::default();
        let mut langs: Vec<&u16> = p.tries.keys().collect();
        langs.sort_unstable();
        for &l in langs {
            let r = put(&mut f, &p.tries[&l]);
            f.roots.push((l, r));
        }
        f.exceptions = p
            .exceptions
            .iter()
            .map(|((l, w), pos)| (word_hash(*l, w), *l, w.clone(), pos.clone()))
            .collect();
        f.exceptions
            .sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
        f
    }

    fn exception(&self, lang: u16, word: &[u16]) -> Option<&[usize]> {
        let h = word_hash(lang, word);
        let at = self.exceptions.partition_point(|e| e.0 < h);
        self.exceptions[at..]
            .iter()
            .take_while(|e| e.0 == h)
            .find(|e| e.1 == lang && e.2 == word)
            .map(|e| e.3.as_slice())
    }

    fn child(&self, z: u32, c: u16) -> Option<u32> {
        let [k0, n, ..] = self.nodes[z as usize];
        let (k0, n) = (k0 as usize, n as usize);
        self.keys[k0..k0 + n]
            .iter()
            .position(|&k| k == c)
            .map(|j| self.kids[k0 + j])
    }

    fn ops(&self, z: u32) -> &[(u8, u8)] {
        let [.., o0, n] = self.nodes[z as usize];
        &self.ops[o0 as usize..(o0 + n) as usize]
    }
}

/// `set_cur_lang` (§934): languages outside 1..=255 are 0.
#[must_use]
pub fn cur_lang(language: i32) -> u16 {
    u16::try_from(language)
        .ok()
        .filter(|l| *l <= 255)
        .unwrap_or(0)
}

impl Patterns {
    /// Enters the pattern `letters` (`hc[1..k]`, 0 for `.`) with values
    /// `digits` (`hyf[0..k]`) for `lang` (§963–§965). Returns whether it
    /// was a duplicate.
    ///
    /// # Panics
    /// If `digits.len() != letters.len() + 1`.
    pub fn add_pattern(&mut self, lang: u16, letters: &[u16], digits: &[u8]) -> bool {
        assert_eq!(digits.len(), letters.len() + 1);
        let k = letters.len();
        let mut digits = digits.to_vec();
        // §965
        if letters.first() == Some(&0) {
            digits[0] = 0;
        }
        if letters.last() == Some(&0) {
            digits[k] = 0;
        }
        self.flat = FlatCache::default();
        let mut node = self.tries.entry(lang).or_default();
        for &c in letters {
            node = node.children.entry(c).or_default();
        }
        let ops: Vec<(usize, u8)> = digits
            .iter()
            .enumerate()
            .filter(|(_, d)| **d != 0)
            .map(|(i, d)| (i, *d))
            .collect();
        // A pattern is "known" once it carries an op; a pattern of zeros
        // stores the null op (§963).
        let duplicate = !node.ops.is_empty();
        node.ops = ops;
        duplicate
    }

    /// One `\patterns` word as `\patterns` reads it (§962): digits are
    /// values unless they follow a digit, `.` is the word edge, other
    /// characters go through `lc` (their `\lccode`). At most 63 letters.
    pub fn add_pattern_str(
        &mut self,
        lang: u16,
        word: &str,
        lc: &dyn Fn(u16) -> u16,
    ) -> PatternReport {
        let mut report = PatternReport::default();
        let mut hc: Vec<u16> = Vec::new();
        let mut hyf: Vec<u8> = vec![0];
        let mut digit_sensed = false;
        for ch in word.chars() {
            let code = u16::try_from(u32::from(ch)).unwrap_or(0);
            if digit_sensed || !ch.is_ascii_digit() {
                let c = if ch == '.' {
                    0
                } else {
                    let c = lc(code);
                    if c == 0 {
                        report.nonletter = true;
                    }
                    c
                };
                if hc.len() < 63 {
                    hc.push(c);
                    hyf.push(0);
                    digit_sensed = false;
                }
            } else if hc.len() < 63 {
                *hyf.last_mut().unwrap_or(&mut 0) =
                    u8::try_from(code - u16::from(b'0')).unwrap_or(0);
                digit_sensed = true;
            }
        }
        if !hc.is_empty() {
            report.duplicate = self.add_pattern(lang, &hc, &hyf);
        }
        report
    }

    /// Enters an exception: the lowercase codes of the word and the
    /// positions `k` of hyphens after `k` letters (§939, §940); a later
    /// entry for the same word replaces the earlier one (web2c).
    pub fn add_exception(&mut self, lang: u16, word: Vec<u16>, positions: Vec<usize>) {
        self.flat = FlatCache::default();
        self.exceptions.insert((lang, word), positions);
    }

    /// One `\hyphenation` word (§935–§939): `-` marks a hyphen, other
    /// characters go through `lc`; nonletters are ignored ("Not a
    /// letter"). Returns false if the character was a nonletter anywhere.
    pub fn add_exception_str(&mut self, lang: u16, word: &str, lc: &dyn Fn(u16) -> u16) -> bool {
        let mut ok = true;
        let mut hc = Vec::new();
        let mut pos = Vec::new();
        for ch in word.chars() {
            if ch == '-' {
                if hc.len() < 63 {
                    pos.push(hc.len());
                }
            } else {
                let c = lc(u16::try_from(u32::from(ch)).unwrap_or(0));
                if c == 0 {
                    ok = false;
                } else if hc.len() < 63 {
                    hc.push(c);
                }
            }
        }
        if hc.len() > 1 {
            self.add_exception(lang, hc, pos);
        }
        ok
    }

    /// e-TeX's `\savinghyphcodes`: the `\lccode` table (indexed by
    /// character) saved with the patterns of `lang`, used instead of the
    /// current `\lccode`s for that language.
    pub fn set_hyph_codes(&mut self, lang: u16, codes: Vec<u16>) {
        self.hyph_codes.insert(lang, codes);
    }

    /// `set_lc_code` (etex.ch): the hyphenation code of `c` in `lang`.
    #[must_use]
    pub fn hyph_code(&self, lang: u16, c: u16, lc: &dyn Fn(u16) -> u16) -> u16 {
        match self.hyph_codes.get(&lang) {
            Some(t) => t.get(usize::from(c)).copied().unwrap_or(0),
            None => lc(c),
        }
    }

    /// §923: the `hyf` values for the word `hc[1..=hn]` (`word`), or
    /// `None` when the language has no patterns and the word is no
    /// exception. `hyf[0..l_hyf]` and the last `r_hyf` values are zero.
    #[must_use]
    pub fn hyphenate_word(
        &self,
        lang: u16,
        word: &[u16],
        l_hyf: usize,
        r_hyf: usize,
    ) -> Option<Vec<u8>> {
        let mut hyf = vec![0u8; word.len() + 1];
        self.hyphenate_word_into(lang, word, l_hyf, r_hyf, &mut hyf)
            .then_some(hyf)
    }

    /// [`Self::hyphenate_word`] into `hyf` (at least `word.len() + 1`
    /// long; all of it is written, zero past the word): whether the
    /// language has patterns or the word is an exception.
    pub fn hyphenate_word_into(
        &self,
        lang: u16,
        word: &[u16],
        l_hyf: usize,
        r_hyf: usize,
        hyf: &mut [u8],
    ) -> bool {
        let hn = word.len();
        hyf.fill(0);
        let hyf = &mut hyf[..=hn];
        let f = self.flat.0.get_or_init(|| Flat::of(self));
        if let Some(pos) = f.exception(lang, word) {
            // §932
            for &p in pos {
                if p <= hn {
                    hyf[p] = 1;
                }
            }
        } else {
            let Some(root) = f.roots.iter().find(|r| r.0 == lang).map(|r| r.1) else {
                return false;
            };
            // (the word between its boundaries: on the stack for the
            // words hyphenation meets, at most 63 letters)
            let mut buf = [0u16; 72];
            let mut heap;
            let hc: &mut [u16] = if hn + 3 <= buf.len() {
                &mut buf[..hn + 3]
            } else {
                heap = vec![0u16; hn + 3];
                &mut heap
            };
            hc[1..=hn].copy_from_slice(word);
            hc[hn + 1] = 0;
            hc[hn + 2] = NON_CHAR;
            for j in 0..=(hn + 1).saturating_sub(r_hyf) {
                let mut z = root;
                let mut l = j;
                while let Some(next) = f.child(z, hc[l]) {
                    z = next;
                    // §924: n_i goes between hc[j+i-1] and hc[j+i]
                    for &(i, v) in f.ops(z) {
                        if let Some(h) = (j + usize::from(i))
                            .checked_sub(1)
                            .and_then(|x| hyf.get_mut(x))
                        {
                            *h = (*h).max(v);
                        }
                    }
                    l += 1;
                }
            }
        }
        for h in hyf.iter_mut().take(l_hyf) {
            *h = 0;
        }
        for j in 0..r_hyf.min(hn + 1) {
            hyf[hn - j] = 0;
        }
        true
    }
}

/// TeX's hyphenation of the word after a glue node in the second pass of
/// line breaking (§894–§918).
pub struct TexHyphenator<'a> {
    /// The patterns and exceptions.
    pub patterns: &'a Patterns,
    /// `\lccode`.
    pub lc_code: &'a dyn Fn(u16) -> u16,
    /// `\uchyph`.
    pub uc_hyph: i32,
    /// `\hyphenchar` of a font.
    pub hyphen_char: &'a dyn Fn(FontId) -> i32,
}

impl Hyphenator for TexHyphenator<'_> {
    #[allow(clippy::too_many_lines)]
    fn hyphenate(
        &mut self,
        list: &[Node],
        at: usize,
        lang: LangState,
        fonts: &dyn Fonts,
    ) -> Option<(usize, usize, List)> {
        let mut lang = lang;
        let code = |lang: u16, c: u16| self.patterns.hyph_code(lang, c, self.lc_code);
        // §896: skip to node ha
        let mut prev_s = at;
        let mut s = at + 1;
        let hf;
        loop {
            let n = list.get(s)?;
            let (c, f) = match n {
                Node::Char { font, ch } => (*ch, *font),
                Node::Lig(l) => match l.original.first() {
                    None => {
                        prev_s = s;
                        s += 1;
                        continue;
                    }
                    Some(&c) => (c, l.font),
                },
                Node::Kern(_, KernKind::Normal) | Node::Whatsit(..) => {
                    prev_s = s;
                    s += 1;
                    continue;
                }
                Node::Language { lang: l, lhm, rhm } => {
                    // §1363
                    lang = LangState {
                        lang: *l,
                        lhm: *lhm,
                        rhm: *rhm,
                    };
                    prev_s = s;
                    s += 1;
                    continue;
                }
                _ => return None,
            };
            let lcc = code(lang.lang, c);
            if lcc != 0 {
                if lcc == c || self.uc_hyph > 0 {
                    hf = f;
                    break;
                }
                return None;
            }
            prev_s = s;
            s += 1;
        }
        // done2
        let hyf_char = u16::try_from((self.hyphen_char)(hf)).ok()?;
        if hyf_char > 255 {
            return None;
        }
        let ha = prev_s;
        let (l_hyf, r_hyf) = (usize::from(lang.lhm), usize::from(lang.rhm));
        if l_hyf + r_hyf > 63 {
            return None;
        }
        let font_bchar = fonts.bchar(hf).unwrap_or(NON_CHAR);
        // §897
        let mut hu = [0u16; 66];
        // (the word's codes, at most 63)
        let mut hc = [0u16; 64];
        let mut hn = 0usize;
        let mut hb = s;
        let mut hyf_bchar = NON_CHAR;
        loop {
            match list.get(s) {
                Some(Node::Char { font, ch }) => {
                    if *font != hf {
                        break;
                    }
                    hyf_bchar = *ch;
                    let c = code(lang.lang, *ch);
                    if c == 0 || hn == 63 {
                        break;
                    }
                    hb = s;
                    hc[hn] = c;
                    hn += 1;
                    hu[hn] = *ch;
                    hyf_bchar = NON_CHAR;
                }
                Some(Node::Lig(l)) => {
                    // §898
                    if l.font != hf {
                        break;
                    }
                    if let Some(&c) = l.original.first() {
                        hyf_bchar = c;
                    }
                    let mut tmp = [(0u16, 0u16); 64];
                    let mut nt = 0usize;
                    let mut all = true;
                    for &c in &l.original {
                        let cc = code(lang.lang, c);
                        if cc == 0 || hn + nt == 63 {
                            all = false;
                            break;
                        }
                        tmp[nt] = (c, cc);
                        nt += 1;
                    }
                    if !all {
                        break;
                    }
                    for &(c, cc) in &tmp[..nt] {
                        hc[hn] = cc;
                        hn += 1;
                        hu[hn] = c;
                    }
                    hb = s;
                    hyf_bchar = if l.right_hit { font_bchar } else { NON_CHAR };
                }
                Some(Node::Kern(_, KernKind::Normal)) => {
                    hb = s;
                    hyf_bchar = font_bchar;
                }
                _ => break,
            }
            s += 1;
        }
        // §899
        if hn < l_hyf + r_hyf {
            return None;
        }
        loop {
            match list.get(s) {
                Some(Node::Char { .. } | Node::Lig(_) | Node::Kern(_, KernKind::Normal)) => s += 1,
                Some(
                    Node::Kern(..)
                    | Node::Whatsit(..)
                    | Node::Language { .. }
                    | Node::Glue(_)
                    | Node::Penalty(_)
                    | Node::Ins(_)
                    | Node::Adjust(_)
                    | Node::Mark(_),
                ) => break,
                _ => return None,
            }
        }
        // §923
        let mut hyf = [0u8; 66];
        if !self
            .patterns
            .hyphenate_word_into(lang.lang, &hc[..hn], l_hyf, r_hyf, &mut hyf)
        {
            return None;
        }
        // §902
        if !(l_hyf..=hn - r_hyf).any(|j| hyf[j] & 1 != 0) {
            return None;
        }
        // §903
        let mut rc = Recon {
            fonts,
            hf,
            hu,
            hyf,
            init_list: &[],
            init_lig: false,
            init_lft: false,
            hyphen_passed: 0,
        };
        let bchar = hyf_bchar;
        let first_after_ha = list.get(ha + 1);
        let (keep_ha, j0) = match &list[ha] {
            Node::Char { font, ch } if *font == hf => {
                rc.init_list = std::slice::from_ref(ch);
                rc.hu[0] = *ch;
                (false, 0)
            }
            Node::Lig(l) if l.font == hf => {
                rc.init_list = &l.original[..];
                rc.init_lig = true;
                rc.init_lft = l.left_hit;
                rc.hu[0] = l.ch;
                if rc.init_list.is_empty() && rc.init_lft {
                    rc.hu[0] = NON_CHAR;
                    rc.init_lig = false;
                }
                (false, 0)
            }
            Node::Char { .. } | Node::Lig(_) => {
                rc.hu[0] = NON_CHAR;
                (true, 0)
            }
            _ => {
                if matches!(first_after_ha, Some(Node::Lig(l)) if l.left_hit) {
                    rc.hu[0] = NON_CHAR;
                    (true, 0)
                } else {
                    (true, 1)
                }
            }
        };
        // §913
        let mut out: List = Vec::new();
        // (a cut's translation before it is known to hold the hyphen;
        // empty at each cut)
        let mut hold: List = Vec::new();
        let mut j = j0;
        loop {
            let mut l = j;
            j = rc.reconstitute(j, hn, bchar, hyf_char, &mut hold) + 1;
            if rc.hyphen_passed == 0 {
                out.append(&mut hold);
                if rc.hyf[j - 1] & 1 != 0 {
                    l = j;
                    rc.hyphen_passed = j - 1;
                }
            }
            if rc.hyphen_passed > 0 {
                // §914
                loop {
                    let mut major = std::mem::take(&mut hold);
                    let mut i = rc.hyphen_passed;
                    rc.hyf[i] = 0;
                    // §915
                    let mut pre = Vec::new();
                    let hyf_node = fonts.char_info(hf, hyf_char).is_some();
                    let mut c = 0;
                    if hyf_node {
                        i += 1;
                        c = rc.hu[i];
                        rc.hu[i] = hyf_char;
                    }
                    while l <= i {
                        l = rc.reconstitute(l, i, font_bchar, NON_CHAR, &mut pre) + 1;
                    }
                    if hyf_node {
                        rc.hu[i] = c;
                        l = i;
                    }
                    // §916
                    let mut post = Vec::new();
                    let mut c_loc = 0;
                    if fonts.has_left_boundary(hf) {
                        l -= 1;
                        c = rc.hu[l];
                        c_loc = l;
                        rc.hu[l] = NON_CHAR;
                    }
                    while l < j {
                        loop {
                            l = rc.reconstitute(l, hn, bchar, NON_CHAR, &mut post) + 1;
                            if c_loc > 0 {
                                rc.hu[c_loc] = c;
                                c_loc = 0;
                            }
                            if l >= j {
                                break;
                            }
                        }
                        while l > j {
                            // §917
                            j = rc.reconstitute(j, hn, bchar, NON_CHAR, &mut major) + 1;
                        }
                    }
                    // §918
                    if major.len() > 127 {
                        out.append(&mut major);
                    } else {
                        out.push(Node::Disc(Disc {
                            pre,
                            post,
                            replace: major,
                        }));
                    }
                    rc.hyphen_passed = j - 1;
                    if rc.hyf[j - 1] & 1 == 0 {
                        break;
                    }
                }
            }
            if j > hn {
                break;
            }
        }
        let from = if keep_ha { ha + 1 } else { ha };
        Some((from, hb, out))
    }
}

/// The state `reconstitute` shares with `hyphenate` (§900–§907).
struct Recon<'a> {
    fonts: &'a dyn Fonts,
    hf: FontId,
    hu: [u16; 66],
    hyf: [u8; 66],
    init_list: &'a [u16],
    init_lig: bool,
    init_lft: bool,
    hyphen_passed: usize,
}

impl Recon<'_> {
    /// `reconstitute(j, n, bchar, hchar)` (§906): the cut prefix ends at
    /// the returned index; its translation is appended to `t`.
    #[allow(clippy::too_many_lines)]
    fn reconstitute(
        &mut self,
        mut j: usize,
        n: usize,
        mut bchar: u16,
        mut hchar: u16,
        t: &mut List,
    ) -> usize {
        let hf = self.hf;
        let ch = |c: u16| Node::Char { font: hf, ch: c };
        self.hyphen_passed = 0;
        let mut w = 0;
        // §908
        let mut cur_l = self.hu[j];
        let mut cur_q = t.len();
        let mut ligature_present = false;
        let mut lft_hit = false;
        let mut rt_hit = false;
        if j == 0 {
            ligature_present = self.init_lig;
            if ligature_present {
                lft_hit = self.init_lft;
            }
            t.extend(self.init_list.iter().map(|&c| ch(c)));
        } else if cur_l < NON_CHAR {
            t.push(ch(cur_l));
        }
        // the ligature stack: (character, the char node of hu[j+1])
        let mut lig_stack: Vec<(u16, Option<u16>)> = Vec::new();
        let set_cur_r = |j: usize, hu: &[u16; 66], hyf: &[u8], bchar: u16, hchar: u16| {
            let cur_r = if j < n { hu[j + 1] } else { bchar };
            let cur_rh = if hyf[j] & 1 != 0 { hchar } else { NON_CHAR };
            (cur_r, cur_rh)
        };
        let (mut cur_r, mut cur_rh) = set_cur_r(j, &self.hu, &self.hyf, bchar, hchar);
        loop {
            // §909
            'done: loop {
                let left = if cur_l == NON_CHAR {
                    Left::Boundary
                } else {
                    Left::Char(cur_l)
                };
                let test_char = if cur_rh < NON_CHAR { cur_rh } else { cur_r };
                let instr = if test_char < NON_CHAR {
                    self.fonts.lig_kern(hf, left, test_char)
                } else {
                    None
                };
                let Some(instr) = instr else {
                    if cur_rh == NON_CHAR {
                        break 'done;
                    }
                    cur_rh = NON_CHAR;
                    continue;
                };
                if cur_rh < NON_CHAR {
                    self.hyphen_passed = j;
                    hchar = NON_CHAR;
                    cur_rh = NON_CHAR;
                    continue;
                }
                if hchar < NON_CHAR && self.hyf[j] & 1 != 0 {
                    self.hyphen_passed = j;
                    hchar = NON_CHAR;
                }
                let (op, rem) = match instr {
                    LigKern::Kern(k) => {
                        w = k;
                        break 'done;
                    }
                    LigKern::Lig(op, rem) => (op, rem),
                };
                // §911
                if cur_l == NON_CHAR {
                    lft_hit = true;
                }
                if j == n && lig_stack.is_empty() {
                    rt_hit = true;
                }
                match op {
                    LigOp::KeepRight | LigOp::KeepRightSkip => {
                        cur_l = rem;
                        ligature_present = true;
                    }
                    LigOp::KeepLeft | LigOp::KeepLeftSkip => {
                        cur_r = rem;
                        if let Some(top) = lig_stack.last_mut() {
                            top.0 = cur_r;
                        } else {
                            let p = if j == n {
                                bchar = NON_CHAR;
                                None
                            } else {
                                Some(self.hu[j + 1])
                            };
                            lig_stack.push((cur_r, p));
                        }
                    }
                    LigOp::KeepBoth => {
                        cur_r = rem;
                        lig_stack.push((cur_r, None));
                    }
                    LigOp::KeepBothSkip1 | LigOp::KeepBothSkip2 => {
                        wrap_lig(
                            t,
                            cur_q,
                            hf,
                            cur_l,
                            &mut lft_hit,
                            false,
                            &lig_stack,
                            &mut ligature_present,
                        );
                        cur_q = t.len();
                        cur_l = rem;
                        ligature_present = true;
                    }
                    LigOp::Plain => {
                        cur_l = rem;
                        ligature_present = true;
                        if !lig_stack.is_empty() {
                            pop_lig_stack(&mut lig_stack, t, &mut j, hf);
                            if let Some(top) = lig_stack.last() {
                                cur_r = top.0;
                            } else {
                                (cur_r, cur_rh) = set_cur_r(j, &self.hu, &self.hyf, bchar, hchar);
                            }
                        } else if j == n {
                            break 'done;
                        } else {
                            t.push(ch(cur_r));
                            j += 1;
                            (cur_r, cur_rh) = set_cur_r(j, &self.hu, &self.hyf, bchar, hchar);
                        }
                    }
                }
                if matches!(
                    op,
                    LigOp::KeepRightSkip | LigOp::KeepLeftSkip | LigOp::KeepBothSkip2
                ) {
                    break 'done;
                }
            }
            // §910
            if wrap_lig(
                t,
                cur_q,
                hf,
                cur_l,
                &mut lft_hit,
                rt_hit,
                &lig_stack,
                &mut ligature_present,
            ) {
                rt_hit = false;
            }
            if w != 0 {
                t.push(Node::Kern(w, KernKind::Normal));
                w = 0;
            }
            let Some(&(top, _)) = lig_stack.last() else {
                break;
            };
            cur_q = t.len();
            cur_l = top;
            ligature_present = true;
            pop_lig_stack(&mut lig_stack, t, &mut j, hf);
            if let Some(top) = lig_stack.last() {
                cur_r = top.0;
            } else {
                (cur_r, cur_rh) = set_cur_r(j, &self.hu, &self.hyf, bchar, hchar);
            }
        }
        j
    }
}

/// `wrap_lig(rt)` (§910); returns whether the right boundary was used.
#[allow(clippy::too_many_arguments)]
fn wrap_lig(
    t: &mut List,
    cur_q: usize,
    hf: FontId,
    cur_l: u16,
    lft_hit: &mut bool,
    rt: bool,
    lig_stack: &[(u16, Option<u16>)],
    ligature_present: &mut bool,
) -> bool {
    if !*ligature_present {
        return false;
    }
    let original = t
        .drain(cur_q..)
        .map(|n| match n {
            Node::Char { ch, .. } => ch,
            _ => unreachable!("only characters follow cur_q"),
        })
        .collect();
    let right_hit = rt && lig_stack.is_empty();
    t.push(Node::Lig(Ligature {
        font: hf,
        ch: cur_l,
        original,
        left_hit: *lft_hit,
        right_hit,
    }));
    *lft_hit = false;
    *ligature_present = false;
    right_hit
}

/// `pop_lig_stack` (§910) without the `cur_r` update, which the caller
/// does.
fn pop_lig_stack(lig_stack: &mut Vec<(u16, Option<u16>)>, t: &mut List, j: &mut usize, hf: FontId) {
    if let Some((_, p)) = lig_stack.pop()
        && let Some(c) = p
    {
        t.push(Node::Char { font: hf, ch: c });
        *j += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lc(c: u16) -> u16 {
        if (0x61..=0x7a).contains(&c) { c } else { 0 }
    }

    fn w(s: &str) -> Vec<u16> {
        s.bytes().map(u16::from).collect()
    }

    #[test]
    fn patterns_take_the_maximum_and_edges() {
        let mut p = Patterns::default();
        assert_eq!(p.add_pattern_str(0, "a1b", &lc), PatternReport::default());
        assert_eq!(p.add_pattern_str(0, "2b", &lc), PatternReport::default());
        // `.` edges: n_0 of a leading `.` is forced to 0
        p.add_pattern_str(0, "1.c3", &lc);
        let hyf = p.hyphenate_word(0, &w("abcab"), 1, 1).unwrap();
        // between a and b: max(1, 2) = 2, even; `.c3` needs a leading c
        assert_eq!(hyf, vec![0, 2, 0, 0, 2, 0]);
        let hyf = p.hyphenate_word(0, &w("cab"), 1, 1).unwrap();
        assert_eq!(hyf, vec![0, 3, 2, 0]);
        assert!(p.add_pattern_str(0, "a3b", &lc).duplicate);
        assert!(p.add_pattern_str(0, "a!b", &lc).nonletter);
        assert_eq!(p.hyphenate_word(1, &w("abc"), 1, 1), None);
    }

    #[test]
    fn exceptions_win_and_later_ones_replace() {
        let mut p = Patterns::default();
        p.add_pattern_str(0, "a1b", &lc);
        assert!(p.add_exception_str(0, "ab-ab", &lc));
        assert_eq!(
            p.hyphenate_word(0, &w("abab"), 1, 1).unwrap(),
            vec![0, 0, 1, 0, 0]
        );
        p.add_exception_str(0, "a-bab", &lc);
        assert_eq!(
            p.hyphenate_word(0, &w("abab"), 1, 1).unwrap(),
            vec![0, 1, 0, 0, 0]
        );
        // exceptions work without patterns for the language
        p.add_exception_str(3, "xy-z", &|c| c);
        assert_eq!(
            p.hyphenate_word(3, &w("xyz"), 1, 1).unwrap(),
            vec![0, 0, 1, 0]
        );
    }

    #[test]
    fn saved_hyph_codes_replace_lccodes() {
        let mut p = Patterns::default();
        p.set_hyph_codes(2, vec![0; 0x62].into_iter().chain([0x61]).collect());
        assert_eq!(p.hyph_code(2, 0x62, &lc), 0x61);
        assert_eq!(p.hyph_code(2, 0x63, &lc), 0);
        assert_eq!(p.hyph_code(0, 0x63, &lc), 0x63);
        assert_eq!(cur_lang(300), 0);
        assert_eq!(cur_lang(-1), 0);
        assert_eq!(cur_lang(7), 7);
    }
}
