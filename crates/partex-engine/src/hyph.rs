//! Hyphenation data: patterns and exceptions (tex.web parts 42–43).
//!
//! Patterns are kept as TeX's packed trie (compact and fast to walk);
//! exceptions as a persistent map from the word (letters by `\lccode`,
//! then the language) to its hyphen positions.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use partex_ssa::{PMap, Value, Version};

/// §920–§921: the packed pattern trie with its hyphenation ops.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Patterns {
    /// `trie_link`, `trie_op`, `trie_char` by trie position.
    pub link: Vec<i32>,
    pub op: Vec<i32>,
    pub ch: Vec<i32>,
    /// `hyf_distance`, `hyf_num`, `hyf_next` by op.
    pub distance: Vec<i32>,
    pub num: Vec<i32>,
    pub next: Vec<i32>,
    /// `op_start` by language.
    pub op_start: Vec<i32>,
    /// e-TeX: by language, the `\lccode`s saved with its patterns
    /// (`\savinghyphcodes`); they replace `\lccode` in hyphenation.
    pub hyph_codes: BTreeMap<u8, Vec<u8>>,
}

crate::persist_struct!(Patterns {
    link,
    op,
    ch,
    distance,
    num,
    next,
    op_start,
    hyph_codes
});

impl Patterns {
    /// The hyphenation code of `c` in `lang`: the saved code if the
    /// language saved codes, else `lc_code`.
    #[must_use]
    pub fn hyph_code(&self, lang: i32, c: u8, lc_code: impl FnOnce(u8) -> u8) -> u8 {
        match u8::try_from(lang)
            .ok()
            .and_then(|l| self.hyph_codes.get(&l))
        {
            Some(codes) => codes[usize::from(c)],
            None => lc_code(c),
        }
    }
}

/// The positions after which a hyphen may go in an exception, a value
/// versioned when it is made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Positions {
    pub at: Vec<u8>,
    ver: Version,
}

impl Positions {
    #[must_use]
    pub fn new(at: Vec<u8>) -> Positions {
        let ver = Version::of(&at);
        Positions { at, ver }
    }
}

impl Value for Positions {
    fn version(&self) -> Version {
        self.ver
    }
}

/// §934: the exception dictionary, a persistent map by word (DESIGN
/// 7.17.12's `hyph` row): a copy is O(1), and its version is made from
/// its entries' as they are written.
#[derive(Clone, Default)]
pub struct Exceptions {
    /// Key: the word's `\lccode`s followed by the language; value: the
    /// positions after which a hyphen may go.
    words: PMap<Vec<u8>, Positions>,
}

impl Exceptions {
    /// The entry of the word `key` ([`exception_key`]), if it is an
    /// exception: its positions and their version.
    #[must_use]
    #[allow(clippy::ptr_arg, reason = "the map's key, looked up as it is")]
    pub fn entry(&self, key: &Vec<u8>) -> Option<&Positions> {
        self.words.get(key)
    }

    /// The version of what the map holds for the word `key`: its
    /// positions', or [`NO_EXCEPTION`] (an O(1) read of the entry).
    #[must_use]
    #[allow(clippy::ptr_arg, reason = "the map's key, looked up as it is")]
    pub fn word_version(&self, key: &Vec<u8>) -> Version {
        entry_version(self.entry(key))
    }

    /// Enter `key` with `positions`: the positions it had, if any.
    pub fn insert(&mut self, key: Vec<u8>, positions: Vec<u8>) -> Option<Vec<u8>> {
        self.words
            .insert(key, Positions::new(positions))
            .map(|p| p.at)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.words.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    pub fn clear(&mut self) {
        self.words = PMap::new();
    }

    /// The map's version, made from its entries' (O(1)).
    #[must_use]
    pub fn version(&self) -> Version {
        self.words.version()
    }

    /// The entries in the order of their words.
    #[must_use]
    pub fn sorted(&self) -> Vec<(&[u8], &[u8])> {
        let mut v: Vec<(&[u8], &[u8])> = self
            .words
            .entries()
            .into_iter()
            .map(|(k, p)| (&k[..], &p.at[..]))
            .collect();
        v.sort_unstable();
        v
    }
}

impl PartialEq for Exceptions {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.sorted() == other.sorted()
    }
}

impl Eq for Exceptions {}

impl core::fmt::Debug for Exceptions {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_map().entries(self.sorted()).finish()
    }
}

impl core::hash::Hash for Exceptions {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        // (by content: the version is made from the entries)
        self.version().0.hash(h);
    }
}

impl crate::persist::Persist for Exceptions {
    fn save(&self, s: &mut crate::persist::Saver) {
        let v: Vec<(Vec<u8>, Vec<u8>)> = self
            .sorted()
            .into_iter()
            .map(|(k, p)| (k.to_vec(), p.to_vec()))
            .collect();
        v.save(s);
    }
    fn load(l: &mut crate::persist::Loader) -> Option<Self> {
        let v: Vec<(Vec<u8>, Vec<u8>)> = crate::persist::Persist::load(l)?;
        let mut e = Exceptions::default();
        for (k, p) in v {
            e.insert(k, p);
        }
        Some(e)
    }
}

/// §930: the key of the word `word` in `lang` in the exception table.
#[must_use]
pub fn exception_key(word: &[u8], lang: i32) -> Vec<u8> {
    let mut key = Vec::with_capacity(word.len() + 1);
    key.extend_from_slice(word);
    key.push(u8::try_from(lang).unwrap_or(0));
    key
}

/// The version of a word that is no exception.
pub const NO_EXCEPTION: Version = Version(0x6e6f_2065_7863_6570_7469_6f6e);

/// The version of what the exception table holds for a word: the
/// positions' version, or [`NO_EXCEPTION`].
#[must_use]
pub fn entry_version(entry: Option<&Positions>) -> Version {
    entry.map_or(NO_EXCEPTION, Value::version)
}

/// §900: the most letters a hyphenated word may have.
pub const MAX_WORD: usize = 63;

/// §923: the hyphenation values of the word `hc[1..=hn]` (`\lccode`s) in
/// `lang`, into `hyf[0..=hn]`; `false` if the language has no patterns and
/// the word is no exception. `exception` is what §930's look for the word
/// in the exception table found.
#[must_use]
pub fn hyphen_values(
    patterns: &Patterns,
    exception: Option<&[u8]>,
    lang: i32,
    word: &[u8],
    r_hyf: usize,
    hyf: &mut [u8; MAX_WORD + 2],
) -> bool {
    let hn = word.len();
    hyf[..=hn].fill(0);
    // §930: the word is in the exception table.
    if let Some(positions) = exception {
        for &i in positions {
            hyf[usize::from(i)] = 1;
        }
        return true;
    }
    let l = usize::try_from(lang).unwrap_or(0);
    if patterns.ch.get(l + 1).copied() != Some(lang) {
        return false; // no patterns for `cur_lang`
    }
    // `hc[0..=hn+2]` with the delimiters.
    let mut hc = [0i32; MAX_WORD + 3];
    for (j, &c) in word.iter().enumerate() {
        hc[j + 1] = i32::from(c);
    }
    hc[hn + 2] = 256;
    let at = |v: &Vec<i32>, i: i32| v[usize::try_from(i).unwrap_or(0)];
    for j in 0..=(hn + 1).saturating_sub(r_hyf) {
        let mut z = at(&patterns.link, lang + 1) + hc[j];
        let mut l = j;
        while hc[l] == at(&patterns.ch, z) {
            if at(&patterns.op, z) != 0 {
                // §924: store maximum values in the `hyf` table.
                let mut v = at(&patterns.op, z);
                loop {
                    v += patterns.op_start[usize::try_from(lang).unwrap_or(0)];
                    let i = l - usize::try_from(at(&patterns.distance, v)).unwrap_or(0);
                    let n = u8::try_from(at(&patterns.num, v)).unwrap_or(0);
                    if n > hyf[i] {
                        hyf[i] = n;
                    }
                    v = at(&patterns.next, v);
                    if v == 0 {
                        break;
                    }
                }
            }
            l += 1;
            z = at(&patterns.link, z) + hc[l];
        }
    }
    true
}
