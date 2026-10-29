//! Glyph origins (docs/note-origins.md): for each code a shipped PDF
//! page shows, the source bytes its glyph came from.
//!
//! - **The writer's glyphs** ([`PageGlyphs`]): the page's PDF side
//!   (`crate::pdfout::PdfPage`) writes the page with a tap
//!   ([`Recorder`], `phitex_pdf::GlyphTap`) while origins are on: one
//!   entry a code its own content stream shows, in the stream's order,
//!   kept by the page's identity in the link ([`keep`], [`kept`]).
//! - **The matcher** ([`match_page`]): the page's glyph text matched
//!   word by word against the text of the source nodes it comes from
//!   (found by a shingle index over every node's text, [`Index`]); each
//!   glyph gets the bytes of the source characters it was matched to,
//!   as `(node, offset in its text)`. A glyph between two matched ones
//!   of a line is synthesized (a macro's: the call's range); the rest
//!   have none. A pure function of the glyphs and the texts: the same at
//!   every thread count.
//! - **Placing** ([`Placer`]): `(node, offset)` to `(file, byte)` in the
//!   file as the Build holds it now (the document's raw text through its
//!   canonical one, a Latin-1 text's characters counted as its bytes).

use std::sync::{Arc, Mutex, OnceLock};

use phitex_defuse::{FxHashMap, hash_bytes, hash_words};
use phitex_pdf::GlyphTap;
use phitex_typeset::arith::Scaled;
use phitex_typeset::fonts::FontId;

use crate::exec::LockWait as _;

// ---- the writer's glyphs ----

/// A code's kind ([`Code::kind`]).
pub(crate) const GLYPH: u8 = 0;
/// A code no character node made.
pub(crate) const NONE: u8 = 1;
/// A form or an included page drawn: its codes counted from the PDF.
pub(crate) const XOBJECT: u8 = 2;

/// One entry of a page's own content stream ([`PageGlyphs`]).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Code {
    pub kind: u8,
    /// The node's first code (a virtual font's character can show more).
    pub first: bool,
    /// The node's character, or where its ligature's characters are in
    /// [`PageGlyphs::lig`] (`lig_n` of them; 0: a character).
    pub ch: u16,
    pub lig_n: u16,
    pub lig: u32,
    /// Position (sp from the page's top left), width and font size.
    pub h: Scaled,
    pub v: Scaled,
    pub w: Scaled,
    pub size: Scaled,
}

/// A page's codes as its writer showed them.
#[derive(Clone, Debug, Default)]
pub(crate) struct PageGlyphs {
    pub codes: Vec<Code>,
    pub lig: Vec<u16>,
}

impl PageGlyphs {
    /// The characters of code `c`'s node.
    fn chars(&self, c: &Code) -> Vec<char> {
        if c.lig_n == 0 {
            return vec![glyph_char(c.ch)];
        }
        let at = c.lig as usize;
        self.lig[at..at + c.lig_n as usize]
            .iter()
            .map(|&x| glyph_char(x))
            .collect()
    }
}

/// A character code as the character it most likely shows: TeX's text
/// fonts keep ASCII's letters, digits and most punctuation at their
/// codes, T1's upper half is Latin-1.
fn glyph_char(c: u16) -> char {
    char::from_u32(u32::from(c)).unwrap_or('\u{FFFD}')
}

/// The tap the page's writer tells its codes ([`PageGlyphs`]), given
/// back through `out` when it is dropped.
pub(crate) struct Recorder {
    g: PageGlyphs,
    out: Arc<Mutex<Option<PageGlyphs>>>,
}

impl Recorder {
    pub(crate) fn new() -> (Recorder, Arc<Mutex<Option<PageGlyphs>>>) {
        let out = Arc::new(Mutex::new(None));
        (
            Recorder {
                g: PageGlyphs::default(),
                out: out.clone(),
            },
            out,
        )
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        *self.out.locked() = Some(std::mem::take(&mut self.g));
    }
}

impl GlyphTap for Recorder {
    fn code(
        &mut self,
        node: (FontId, u16),
        lig: &[u16],
        first: bool,
        _f: FontId,
        _c: u8,
        h: Scaled,
        v: Scaled,
        w: Scaled,
        size: Scaled,
    ) {
        let at = u32::try_from(self.g.lig.len()).unwrap_or(u32::MAX);
        if first && !lig.is_empty() {
            self.g.lig.extend_from_slice(lig);
        }
        self.g.codes.push(Code {
            kind: GLYPH,
            first,
            ch: node.1,
            lig_n: if first {
                u16::try_from(lig.len()).unwrap_or(0)
            } else {
                0
            },
            lig: at,
            h,
            v,
            w,
            size,
        });
    }

    fn none(&mut self, n: usize) {
        for _ in 0..n {
            self.g.codes.push(Code {
                kind: NONE,
                ..Code::default()
            });
        }
    }

    fn literal(&mut self, bytes: &[u8]) {
        struct Count(usize);
        impl phitex_codecs::pdftext::Text for Count {
            fn font(&mut self, _: &[u8]) -> phitex_codecs::pdftext::Font {
                phitex_codecs::pdftext::Font::default()
            }
            fn code(&mut self, _: &[u8], _: u32, _: f64, _: f64) {
                self.0 += 1;
            }
            fn draw(&mut self, _: &[u8], _: &phitex_codecs::pdftext::State) {}
        }
        // (a literal showing text: rare; most set colours)
        if !bytes.iter().any(|&b| b == b'T' || b == b'\'' || b == b'"') {
            return;
        }
        let mut c = Count(0);
        phitex_codecs::pdftext::walk(bytes, &mut c);
        self.none(c.0);
    }

    fn xobject(&mut self) {
        self.g.codes.push(Code {
            kind: XOBJECT,
            ..Code::default()
        });
    }

    /// A native word's glyphs: the first carries the word's text (as a
    /// ligature its characters), the others are its node's further
    /// codes.
    fn native(
        &mut self,
        _font: FontId,
        text: &[u16],
        glyphs: &[(Scaled, Scaled, Scaled)],
        size: Scaled,
    ) {
        // (a glyph with no text, `\XeTeXglyph`, a math font's: a glyph
        // no character reads as)
        if text.is_empty() {
            for &(h, v, w) in glyphs {
                self.g.codes.push(Code {
                    kind: GLYPH,
                    first: true,
                    ch: 0xFFFD,
                    h,
                    v,
                    w,
                    size,
                    ..Code::default()
                });
            }
            return;
        }
        let at = u32::try_from(self.g.lig.len()).unwrap_or(u32::MAX);
        if !glyphs.is_empty() {
            self.g.lig.extend_from_slice(text);
        }
        for (i, &(h, v, w)) in glyphs.iter().enumerate() {
            self.g.codes.push(Code {
                kind: GLYPH,
                first: i == 0,
                ch: text[0],
                lig_n: if i == 0 {
                    u16::try_from(text.len()).unwrap_or(u16::MAX)
                } else {
                    0
                },
                lig: at,
                h,
                v,
                w,
                size,
            });
        }
    }
}

/// The pages' glyphs by the value store and the page's identity in the
/// link (`crate::pdfout::Linked::shown`).
type GlyphMemo = Mutex<FxHashMap<(u64, u64), Arc<PageGlyphs>>>;

fn glyph_memo() -> &'static GlyphMemo {
    static M: OnceLock<GlyphMemo> = OnceLock::new();
    M.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// Page `id`'s glyphs (value store `inst`), kept.
pub(crate) fn keep(inst: u64, id: u64, g: PageGlyphs) {
    let mut m = glyph_memo().locked();
    // (the pages of builds gone: dropped wholesale past a bound, made
    // again where a page's writer runs)
    if m.len() > 1 << 16 {
        m.clear();
    }
    m.insert((inst, id), Arc::new(g));
}

/// Page `id`'s glyphs (value store `inst`), if kept.
pub(crate) fn kept(inst: u64, id: u64) -> Option<Arc<PageGlyphs>> {
    glyph_memo().locked().get(&(inst, id)).cloned()
}

/// Whether page `id` (value store `inst`) has its glyphs kept.
pub(crate) fn has(inst: u64, id: u64) -> bool {
    glyph_memo().locked().contains_key(&(inst, id))
}

// ---- the sources ----

/// A text node's text as the matcher reads it.
#[derive(Debug)]
pub(crate) struct NodeScan {
    /// The hash of its text.
    pub text_hash: u64,
    /// Its candidate characters: folded, and where they are (byte in
    /// the node's text, length).
    cands: Vec<(char, u32, u32)>,
    /// Its words: candidate ranges and their folded hashes.
    words: Vec<(u32, u32, u64)>,
    /// Its control sequences' byte ranges (`\\LaTeX`), in order.
    calls: Vec<(u32, u32)>,
}

/// A character as the matcher reads it: the dashes and quotes TeX's
/// ligatures make (`---`, `--`, ` `` `, `''`) as the characters they are
/// typed with, on both sides (XeTeX's `tex-text` mapping puts `—` in a
/// native word where the source has `---`, or the source has `—`
/// itself), each folded ([`fold`]).
fn folded(c: char) -> &'static [char] {
    match c {
        '\u{2014}' => &['-', '-', '-'],
        '\u{2013}' => &['-', '-'],
        '\u{201C}' => &['`', '`'],
        '\u{201D}' => &['\'', '\''],
        '\u{2018}' => &['`'],
        '\u{2019}' => &['\''],
        _ => &[],
    }
}

/// Character `c` read ([`folded`], [`fold`]), each told `f`.
fn each_folded(c: char, mut f: impl FnMut(char)) {
    match folded(c) {
        [] => f(fold(c)),
        cs => cs.iter().for_each(|&x| f(x)),
    }
}

/// Latin-1 accented letters folded to their base letter.
fn fold(c: char) -> char {
    const UP: &str = "AAAAAAACEEEEIIIIDNOOOOOxOUUUUYTs";
    const LO: &str = "aaaaaaaceeeeiiiidnooooo/ouuuuyty";
    let u = u32::from(c);
    match u {
        0xC0..=0xDF => UP
            .as_bytes()
            .get((u - 0xC0) as usize)
            .map_or(c, |&b| char::from(b)),
        0xE0..=0xFF => LO
            .as_bytes()
            .get((u - 0xE0) as usize)
            .map_or(c, |&b| char::from(b)),
        0x131 => 'i',
        _ => c,
    }
}

/// The scan of a node's text `t`: TeX's reading of a document, enough
/// to know which characters can be a glyph's.
#[allow(clippy::too_many_lines)]
pub(crate) fn scan(t: &str) -> NodeScan {
    let cs: Vec<(usize, char)> = t.char_indices().collect();
    let latin = cs.iter().all(|&(_, c)| u32::from(c) <= 0xFF);
    let mut cands: Vec<(char, u32, u32)> = Vec::new();
    let mut words = Vec::new();
    let mut calls = Vec::new();
    let mut wstart = 0usize;
    let at =
        |i: usize| -> u32 { u32::try_from(cs.get(i).map_or(t.len(), |x| x.0)).unwrap_or(u32::MAX) };
    let brk =
        |cands: &Vec<(char, u32, u32)>, wstart: &mut usize, words: &mut Vec<(u32, u32, u64)>| {
            if cands.len() > *wstart {
                let h = cands[*wstart..].iter().fold(0x776f_7264_u64, |h, c| {
                    hash_words(h, &[u64::from(u32::from(c.0))])
                });
                words.push((
                    u32::try_from(*wstart).unwrap_or(0),
                    u32::try_from(cands.len()).unwrap_or(0),
                    h,
                ));
            }
            *wstart = cands.len();
        };
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i].1;
        match c {
            '%' => {
                while i < cs.len() && cs[i].1 != '\n' {
                    i += 1;
                }
                brk(&cands, &mut wstart, &mut words);
            }
            '\\' => {
                let Some(&(_, n)) = cs.get(i + 1) else {
                    i += 1;
                    continue;
                };
                if n.is_ascii_alphabetic() || n == '@' {
                    let mut j = i + 1;
                    while j < cs.len() && (cs[j].1.is_ascii_alphabetic() || cs[j].1 == '@') {
                        j += 1;
                    }
                    calls.push((at(i), at(j)));
                    brk(&cands, &mut wstart, &mut words);
                    i = j;
                    continue;
                }
                if "%&$#_{}".contains(n) {
                    cands.push((n, at(i + 1), at(i + 2) - at(i + 1)));
                } else {
                    calls.push((at(i), at(i + 2)));
                    if !"'\"^~`=.-/".contains(n) {
                        brk(&cands, &mut wstart, &mut words);
                    }
                }
                i += 2;
                continue;
            }
            // (a group's brace: no space; but a symbol before an
            // argument's brace, `\footnote*{`, ends its word)
            '{' if cands.len() > wstart && !cands[cands.len() - 1].0.is_alphanumeric() => {
                brk(&cands, &mut wstart, &mut words);
            }
            '{' | '}' => {}
            '$' | '&' | '#' | '^' | '_' | '~' => brk(&cands, &mut wstart, &mut words),
            c if c.is_whitespace() || c == '\0' => brk(&cands, &mut wstart, &mut words),
            _ => {
                // (a UTF-8 sequence in a Latin-1 text: one character)
                let u = u32::from(c);
                let n = match u {
                    0xC2..=0xDF => 2,
                    0xE0..=0xEF => 3,
                    0xF0..=0xF4 => 4,
                    _ => 1,
                };
                if latin
                    && n > 1
                    && i + n <= cs.len()
                    && cs[i + 1..i + n]
                        .iter()
                        .all(|x| (0x80..=0xBF).contains(&u32::from(x.1)))
                {
                    let bytes: Vec<u8> = cs[i..i + n]
                        .iter()
                        .map(|x| u8::try_from(u32::from(x.1)).unwrap_or(0))
                        .collect();
                    let ch = std::str::from_utf8(&bytes)
                        .ok()
                        .and_then(|s| s.chars().next())
                        .unwrap_or(c);
                    each_folded(ch, |x| cands.push((x, at(i), at(i + n) - at(i))));
                    i += n;
                    continue;
                }
                each_folded(c, |x| cands.push((x, at(i), at(i + 1) - at(i))));
            }
        }
        i += 1;
    }
    brk(&cands, &mut wstart, &mut words);
    NodeScan {
        text_hash: hash_bytes(0x6f72_6967, t.as_bytes()),
        cands,
        words,
        calls,
    }
}

/// A source node as the matcher sees it: its id, its place in the
/// document's order, its scan.
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub raw: u64,
    pub scan: Arc<NodeScan>,
}

/// The shingle index over the nodes' words: 4 words in a row (and, for
/// pages too short for those, single words of four characters or more),
/// to the nodes that have them (at most [`CAP`]; more: common).
#[derive(Default)]
pub(crate) struct Index {
    pub nodes: Vec<Node>,
    shingles: FxHashMap<u64, Vec<u32>>,
    singles: FxHashMap<u64, Vec<u32>>,
}

const CAP: usize = 8;

fn shingle(ws: &[u64]) -> u64 {
    hash_words(0x7368_696e, ws)
}

/// The shingles of five words in a row with one of the middle three
/// left out.
fn skip_one(s: &[u64]) -> [u64; 3] {
    [1, 2, 3].map(|k| {
        let mut w = [0u64; 4];
        let mut i = 0;
        for (j, &x) in s.iter().enumerate() {
            if j != k {
                w[i] = x;
                i += 1;
            }
        }
        shingle(&w)
    })
}

impl Index {
    pub(crate) fn new(nodes: Vec<Node>) -> Index {
        let mut ix = Index {
            nodes,
            ..Index::default()
        };
        for (k, n) in ix.nodes.iter().enumerate() {
            let k = u32::try_from(k).unwrap_or(u32::MAX);
            let hs: Vec<u64> = n.scan.words.iter().map(|w| w.2).collect();
            for s in hs.windows(4) {
                add(&mut ix.shingles, shingle(s), k);
            }
            // (and four of five in a row, the middle one left out: a word
            // no page shows, a label's, between them)
            for s in hs.windows(5) {
                for x in skip_one(s) {
                    add(&mut ix.shingles, x, k);
                }
            }
            for w in &n.scan.words {
                if w.1 - w.0 >= 4 {
                    add(&mut ix.singles, w.2, k);
                }
            }
        }
        ix
    }
}

fn add(m: &mut FxHashMap<u64, Vec<u32>>, h: u64, k: u32) {
    let e = m.entry(h).or_default();
    if e.last() != Some(&k) && e.len() <= CAP {
        e.push(k);
    }
}

// ---- the matcher ----

/// A code's origin: a node of the match's ([`Matched::nodes`]), the
/// byte range in its text, and whether it is synthesized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Org {
    pub node: u32,
    pub from: u32,
    pub to: u32,
    pub synth: bool,
}

pub(crate) const NO_NODE: u32 = u32::MAX;

impl Org {
    const NONE: Org = Org {
        node: NO_NODE,
        from: 0,
        to: 0,
        synth: false,
    };
}

/// A page's match: per code, its origin in a node's text.
#[derive(Debug)]
pub(crate) struct Matched {
    /// The nodes it names: raw id and text hash.
    pub nodes: Vec<(u64, u64)>,
    pub orgs: Vec<Org>,
}

/// A glyph of the page as the matcher reads it: its first code's index,
/// its characters' range in the atoms.
struct Glyph {
    code: usize,
    h: Scaled,
    v: Scaled,
    w: Scaled,
    size: Scaled,
}

/// A word's character, folded.
#[derive(Clone, Copy)]
struct Atom {
    ch: char,
}

/// The source words of the nodes matched against: (node, word index).
struct Src<'a> {
    ix: &'a Index,
    words: Vec<(u32, u32)>,
}

impl Src<'_> {
    fn hash(&self, i: usize) -> u64 {
        let (n, w) = self.words[i];
        self.ix.nodes[n as usize].scan.words[w as usize].2
    }
    fn cands(&self, i: usize) -> (u32, &[(char, u32, u32)]) {
        let (n, w) = self.words[i];
        let s = &self.ix.nodes[n as usize].scan;
        let (a, b, _) = s.words[w as usize];
        (n, &s.cands[a as usize..b as usize])
    }
}

/// The match of page `g` against the nodes of `ix`.
#[allow(clippy::too_many_lines)]
pub(crate) fn match_page(g: &PageGlyphs, ix: &Index) -> Matched {
    // the glyphs, one a node (its first code)
    let mut glyphs: Vec<Glyph> = Vec::new();
    let mut atoms: Vec<Atom> = Vec::new();
    // (each glyph's atoms: a range)
    let mut glyph_atoms: Vec<(u32, u32)> = Vec::new();
    for (i, c) in g.codes.iter().enumerate() {
        if c.kind != GLYPH {
            continue;
        }
        // (a node's further codes: its extent to their end, a native
        // word's glyphs after its first)
        if !c.first {
            if let Some(gl) = glyphs.last_mut()
                && c.v == gl.v
                && c.h + c.w > gl.h + gl.w
            {
                gl.w = c.h + c.w - gl.h;
            }
            continue;
        }
        let a = u32::try_from(atoms.len()).unwrap_or(u32::MAX);
        for ch in g.chars(c) {
            each_folded(ch, |x| atoms.push(Atom { ch: x }));
        }
        glyph_atoms.push((a, u32::try_from(atoms.len()).unwrap_or(u32::MAX)));
        glyphs.push(Glyph {
            code: i,
            h: c.h,
            v: c.v,
            w: c.w,
            size: c.size,
        });
    }
    // the page's words: atom lists, a hyphen at a line's end joining
    // the word to the next line's first
    let mut words: Vec<Vec<u32>> = Vec::new();
    let mut cur: Vec<u32> = Vec::new();
    for (k, gl) in glyphs.iter().enumerate() {
        if k > 0 {
            let p = &glyphs[k - 1];
            let line = (gl.v - p.v).abs() > p.size.max(gl.size) / 4;
            let gap = gl.h > p.h + p.w + p.size.max(1) / 6 || gl.h < p.h - p.size.max(1) / 2;
            if line || gap {
                // (the next line's: just below, not a page's foot)
                // (a glyph that is a hyphen alone: not a dash, read as
                // hyphens too, [`folded`])
                let (ha, hb) = glyph_atoms[k - 1];
                let hyphen = line
                    && cur.len() > 1
                    && hb == ha + 1
                    && atoms[ha as usize].ch == '-'
                    && gl.v > p.v
                    && (gl.v - p.v) * 5 <= 8 * p.size.max(gl.size);
                if hyphen {
                    cur.pop();
                } else if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
            }
        }
        let (a, b) = glyph_atoms[k];
        cur.extend(a..b);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    let whash: Vec<u64> = words
        .iter()
        .map(|w| {
            w.iter().fold(0x776f_7264_u64, |h, &a| {
                hash_words(h, &[u64::from(u32::from(atoms[a as usize].ch))])
            })
        })
        .collect();
    // which nodes: the shingles' votes
    let mut votes: FxHashMap<u32, f64> = FxHashMap::default();
    // (four words in a row, and four of five with one of the middle
    // left out: a run of glyphs no source word gives, a cross-reference's
    // `??`, a counter, between them)
    let page_shingles = whash
        .windows(4)
        .map(shingle)
        .chain(whash.windows(5).flat_map(skip_one));
    for sh in page_shingles {
        if let Some(ns) = ix.shingles.get(&sh)
            && ns.len() <= CAP
        {
            #[allow(clippy::cast_precision_loss)]
            let w = 1.0 / ns.len() as f64;
            for &n in ns {
                *votes.entry(n).or_default() += w;
            }
        }
    }
    if votes.is_empty() {
        for h in &whash {
            if let Some(ns) = ix.singles.get(h)
                && ns.len() <= 2
            {
                #[allow(clippy::cast_precision_loss)]
                let w = 1.0 / ns.len() as f64;
                for &n in ns {
                    *votes.entry(n).or_default() += w;
                }
            }
        }
    }
    let mut by_node: Vec<(u32, f64)> = votes.into_iter().collect();
    by_node.sort_unstable_by_key(|x| x.0);
    // (clusters of nodes near one another in the document's order)
    let mut clusters: Vec<(u32, u32, f64)> = Vec::new();
    for &(n, w) in &by_node {
        match clusters.last_mut() {
            Some(c) if n <= c.1 + 3 => {
                c.1 = n;
                c.2 += w;
            }
            _ => clusters.push((n, n, w)),
        }
    }
    let last = u32::try_from(ix.nodes.len()).unwrap_or(0).saturating_sub(1);
    let mut order: Vec<usize> = (0..clusters.len()).collect();
    // (the heaviest first; ties to the earlier)
    order.sort_by(|&a, &b| clusters[b].2.total_cmp(&clusters[a].2).then(a.cmp(&b)));
    let debug = crate::exec::env_flag!("PHITEX_ORIGINS_DEBUG");
    if debug {
        eprintln!(
            "origins: {} glyphs, {} words, clusters {clusters:?}",
            glyphs.len(),
            words.len()
        );
    }
    let mut matched_atoms: Vec<Option<(u32, u32, u32)>> = vec![None; atoms.len()];
    let mut word_done = vec![false; words.len()];
    let mut src_used: phitex_defuse::FxHashSet<(u32, u32)> = phitex_defuse::FxHashSet::default();
    // (the heaviest cluster with every word of the page; then it and the
    // other clusters again with the runs of words left: floats,
    // footnotes, captions, a title; a later pass again while it matches
    // runs, the page's order no longer theirs: a footnote after the
    // text that follows its mark)
    let mut passes: Vec<usize> = order
        .first()
        .into_iter()
        .copied()
        .chain(order.iter().copied())
        .collect();
    let mut round = 0;
    while round < passes.len() {
        let ci = passes[round];
        let (a, b, _) = clusters[ci];
        // (and the nodes around it too short to vote: a heading, an
        // environment's line)
        let short = |n: u32| ix.nodes[n as usize].scan.words.len() < 4;
        let (mut a, mut b) = (a.saturating_sub(1), (b + 1).min(last));
        for _ in 0..3 {
            if a > 0 && short(a) {
                a -= 1;
            }
            if b < last && short(b) {
                b += 1;
            }
        }
        let mut src = Src {
            ix,
            words: Vec::new(),
        };
        for n in a..=b {
            for k in 0..ix.nodes[n as usize].scan.words.len() {
                let k = u32::try_from(k).unwrap_or(0);
                // (a later pass: every word again, a run of three taking
                // it over from a word matched alone)
                if round > 0 || !src_used.contains(&(n, k)) {
                    src.words.push((n, k));
                }
            }
        }
        // (the page's words not matched yet: all of them first, then the
        // runs of three or more left)
        let free: Vec<usize> = (0..words.len()).filter(|&i| !word_done[i]).collect();
        if free.is_empty() {
            break;
        }
        let pa: Vec<u64> = free.iter().map(|&i| whash[i]).collect();
        let pb: Vec<u64> = (0..src.words.len()).map(|i| src.hash(i)).collect();
        let mut pairs = Vec::new();
        diff(
            &pa,
            &pb,
            0,
            pa.len(),
            0,
            pb.len(),
            &mut pairs,
            0,
            true,
            true,
        );
        let keep_run = |pairs: &[(usize, usize)]| -> Vec<(usize, usize)> {
            if round == 0 {
                return pairs.to_vec();
            }
            // (a later cluster: only runs of three words or more in a row)
            let mut out = Vec::new();
            let mut i = 0;
            while i < pairs.len() {
                let mut j = i + 1;
                while j < pairs.len()
                    && pairs[j].0 == pairs[j - 1].0 + 1
                    && pairs[j].1 == pairs[j - 1].1 + 1
                {
                    j += 1;
                }
                if j - i >= 3 {
                    out.extend_from_slice(&pairs[i..j]);
                }
                i = j;
            }
            out
        };
        if debug {
            eprintln!(
                "origins: round {round} cluster {:?} nodes {a}..={b}: {} free, {} src words, {} pairs",
                clusters[ci],
                pa.len(),
                pb.len(),
                pairs.len()
            );
        }
        let pairs = keep_run(&pairs);
        if round > 0 && !pairs.is_empty() {
            passes.insert(round + 1, ci);
        }
        for &(i, j) in &pairs {
            let wi = free[i];
            word_done[wi] = true;
            src_used.insert(src.words[j]);
            let (n, cs) = src.cands(j);
            for (&a, c) in words[wi].iter().zip(cs) {
                matched_atoms[a as usize] = Some((n, c.1, c.1 + c.2));
            }
        }
        // the gaps between matched words: characters by LCS
        if round == 0 {
            let mut prev: Option<(usize, usize)> = None;
            let mut bounds: Vec<((usize, usize), (usize, usize))> = Vec::new();
            for &(i, j) in pairs.iter().chain(std::iter::once(&(pa.len(), pb.len()))) {
                let (pi, pj) = prev.map_or((0, 0), |(a, b)| (a + 1, b + 1));
                if i > pi && j > pj {
                    bounds.push(((pi, i), (pj, j)));
                }
                prev = Some((i, j));
            }
            for ((ia, ib), (ja, jb)) in bounds {
                let mut ga: Vec<u32> = (ia..ib)
                    .flat_map(|i| words[free[i]].iter().copied())
                    .collect();
                let mut sb: Vec<(u32, (char, u32, u32))> = (ja..jb)
                    .flat_map(|j| {
                        let (n, cs) = src.cands(j);
                        cs.iter().map(move |&c| (n, c))
                    })
                    .collect();
                // (the page's first and last gaps: next to their anchor;
                // a gap between two anchors too long for characters: left)
                let (lead, trail) = (ia == 0 && ja == 0, ib == pa.len() && jb == pb.len());
                if trail {
                    ga.truncate(256);
                    sb.truncate(256);
                } else if lead {
                    ga.drain(..ga.len().saturating_sub(256));
                    sb.drain(..sb.len().saturating_sub(256));
                }
                if ga.is_empty() || sb.is_empty() || ga.len() > 256 || sb.len() > 256 {
                    continue;
                }
                let xa: Vec<u64> = ga
                    .iter()
                    .map(|&a| u64::from(u32::from(atoms[a as usize].ch)))
                    .collect();
                let xb: Vec<u64> = sb.iter().map(|x| u64::from(u32::from(x.1.0))).collect();
                // (the page's first gap matched as near its anchor as it
                // can be: from the end)
                let pairs = if lead && !trail {
                    let ra: Vec<u64> = xa.iter().rev().copied().collect();
                    let rb: Vec<u64> = xb.iter().rev().copied().collect();
                    lcs(&ra, &rb)
                        .into_iter()
                        .map(|(i, j)| (xa.len() - 1 - i, xb.len() - 1 - j))
                        .collect()
                } else {
                    lcs(&xa, &xb)
                };
                for (i, j) in pairs {
                    let (n, c) = sb[j];
                    matched_atoms[ga[i] as usize] = Some((n, c.1, c.1 + c.2));
                }
            }
        }
        round += 1;
    }
    // the characters left between two matched ones of one node: matched
    // to that node's characters between them (`footnote*` against
    // `footnote\footnote*`, a word a macro split)
    let mut k = 0;
    while k < atoms.len() {
        if matched_atoms[k].is_some() {
            k += 1;
            continue;
        }
        let s0 = k;
        while k < atoms.len() && matched_atoms[k].is_none() {
            k += 1;
        }
        let p = s0.checked_sub(1).and_then(|x| matched_atoms[x]);
        let q = matched_atoms.get(k).copied().flatten();
        if let (Some(p), Some(q)) = (p, q)
            && p.0 == q.0
            && p.2 <= q.1
            && k - s0 <= 256
        {
            let sc = &ix.nodes[p.0 as usize].scan;
            let lo = sc.cands.partition_point(|c| c.1 < p.2);
            let hi = sc.cands.partition_point(|c| c.1 < q.1);
            if hi > lo && hi - lo <= 512 {
                let xa: Vec<u64> = (s0..k).map(|x| u64::from(u32::from(atoms[x].ch))).collect();
                let xb: Vec<u64> = sc.cands[lo..hi]
                    .iter()
                    .map(|c| u64::from(u32::from(c.0)))
                    .collect();
                for (i, j) in lcs(&xa, &xb) {
                    let c = sc.cands[lo + j];
                    matched_atoms[s0 + i] = Some((p.0, c.1, c.1 + c.2));
                }
            }
        }
    }
    if debug {
        for (i, w) in words.iter().enumerate() {
            let t: String = w.iter().map(|&a| atoms[a as usize].ch).collect();
            let m = w
                .iter()
                .filter(|&&a| matched_atoms[a as usize].is_some())
                .count();
            eprintln!(
                "origins: word {i} {t:?} done {} chars matched {m}/{}",
                word_done[i],
                w.len()
            );
        }
    }
    // the nodes named, in the index's order
    let mut used: Vec<u32> = matched_atoms.iter().flatten().map(|x| x.0).collect();
    used.sort_unstable();
    used.dedup();
    let local = |n: u32| u32::try_from(used.binary_search(&n).unwrap_or(0)).unwrap_or(0);
    // each glyph: its atoms' range, or synthesized between matched ones
    let mut gorg: Vec<Org> = glyph_atoms
        .iter()
        .map(|&(a, b)| {
            let ms: Vec<(u32, u32, u32)> =
                (a..b).filter_map(|i| matched_atoms[i as usize]).collect();
            match (ms.first(), ms.last()) {
                (Some(f), Some(l)) if f.0 == l.0 => Org {
                    node: f.0,
                    from: f.1.min(l.1),
                    to: f.2.max(l.2),
                    synth: false,
                },
                (Some(f), _) => Org {
                    node: f.0,
                    from: f.1,
                    to: f.2,
                    synth: false,
                },
                _ => Org::NONE,
            }
        })
        .collect();
    synthesize(&glyphs, &mut gorg, ix);
    let mut orgs = vec![Org::NONE; g.codes.len()];
    let mut cur = Org::NONE;
    let mut gi = 0usize;
    for (i, c) in g.codes.iter().enumerate() {
        if c.kind != GLYPH {
            continue;
        }
        if c.first {
            cur = gorg.get(gi).copied().unwrap_or(Org::NONE);
            debug_assert_eq!(glyphs.get(gi).map(|x| x.code), Some(i));
            gi += 1;
        }
        orgs[i] = if cur.node == NO_NODE {
            Org::NONE
        } else {
            Org {
                node: local(cur.node),
                ..cur
            }
        };
    }
    Matched {
        nodes: used
            .iter()
            .map(|&n| {
                (
                    ix.nodes[n as usize].raw,
                    ix.nodes[n as usize].scan.text_hash,
                )
            })
            .collect(),
        orgs,
    }
}

/// Unmatched glyphs between two matched ones of the same node on their
/// line: synthesized, with the source between them narrowed to its
/// first control sequence (a macro's call); `None` where there is none.
fn synthesize(glyphs: &[Glyph], gorg: &mut [Org], ix: &Index) {
    let mut k = 0;
    while k < glyphs.len() {
        if gorg[k].node != NO_NODE {
            k += 1;
            continue;
        }
        let mut e = k;
        while e < glyphs.len() && gorg[e].node == NO_NODE {
            e += 1;
        }
        let same_line = |a: usize, b: usize| {
            (glyphs[a].v - glyphs[b].v).abs() <= glyphs[a].size.max(glyphs[b].size)
        };
        let before = (k > 0 && same_line(k - 1, k)).then(|| gorg[k - 1]);
        let after = (e < glyphs.len() && same_line(e, e - 1)).then(|| gorg[e]);
        let calls = |n: u32| &ix.nodes[n as usize].scan.calls;
        let first_call = |n: u32, a: u32, b: u32| {
            calls(n)
                .iter()
                .find(|c| c.0 >= a && c.1 <= b)
                .map(|c| (n, c.0, c.1))
        };
        let last_call = |n: u32, a: u32, b: u32| {
            calls(n)
                .iter()
                .rev()
                .find(|c| c.0 >= a && c.1 <= b)
                .map(|c| (n, c.0, c.1))
        };
        let call = match (before, after) {
            (Some(p), Some(n)) if p.node == n.node && p.to <= n.from => {
                first_call(n.node, p.to, n.from)
            }
            (Some(p), _) => first_call(p.node, p.to, p.to + 64),
            (None, Some(n)) => last_call(n.node, n.from.saturating_sub(64), n.from),
            _ => None,
        };
        let o = match (call, before, after) {
            (Some((n, a, b)), _, _) => Org {
                node: n,
                from: a,
                to: b,
                synth: true,
            },
            (None, Some(p), Some(n)) if p.node == n.node && p.to < n.from => Org {
                node: p.node,
                from: p.to,
                to: n.from,
                synth: true,
            },
            _ => Org::NONE,
        };
        // (a run with no glyph of its line placed, a page's number, a
        // running head, a heading's counter on a line of its own: the
        // call after the glyph placed before it in the page's order, or
        // before the one after it; else that place itself)
        let o = if o.node != NO_NODE {
            o
        } else if k > 0 && gorg[k - 1].node != NO_NODE {
            let p = gorg[k - 1];
            first_call(p.node, p.to, p.to + 64).map_or(
                Org {
                    node: p.node,
                    from: p.to,
                    to: p.to,
                    synth: true,
                },
                |(n, a, b)| Org {
                    node: n,
                    from: a,
                    to: b,
                    synth: true,
                },
            )
        } else if e < glyphs.len() && gorg[e].node != NO_NODE {
            let n = gorg[e];
            last_call(n.node, n.from.saturating_sub(64), n.from).map_or(
                Org {
                    node: n.node,
                    from: n.from,
                    to: n.from,
                    synth: true,
                },
                |(m, a, b)| Org {
                    node: m,
                    from: a,
                    to: b,
                    synth: true,
                },
            )
        } else {
            Org::NONE
        };
        for x in &mut gorg[k..e] {
            *x = o;
        }
        k = e;
    }
}

/// Patience diff of `a[a0..a1]` against `b[b0..b1]`: the pairs matched,
/// in order (unique elements of both as anchors, recursively; small
/// gaps by LCS, large ones greedily). `lead`: the range is the page's
/// start (before its first anchor), whose text may stand earlier in the
/// sources too (a paragraph that repeats a sentence, the page before's
/// part of it): matched from its end, as near the anchor as it can be;
/// `trail`: the page's end, matched from its start.
#[allow(clippy::too_many_arguments)]
fn diff(
    a: &[u64],
    b: &[u64],
    mut a0: usize,
    mut a1: usize,
    mut b0: usize,
    mut b1: usize,
    out: &mut Vec<(usize, usize)>,
    depth: u32,
    lead: bool,
    trail: bool,
) {
    // (common prefix and suffix, where the range is anchored there)
    while !lead && a0 < a1 && b0 < b1 && a[a0] == b[b0] {
        out.push((a0, b0));
        a0 += 1;
        b0 += 1;
    }
    let mut suffix = Vec::new();
    while !trail && a0 < a1 && b0 < b1 && a[a1 - 1] == b[b1 - 1] {
        a1 -= 1;
        b1 -= 1;
        suffix.push((a1, b1));
    }
    if a0 < a1 && b0 < b1 {
        // (unique in both: their positions; where no word is, three
        // words in a row)
        let unique = |n: usize| -> Vec<(usize, usize)> {
            let key = |xs: &[u64], i: usize| hash_words(0x7472_6967, &xs[i..i + n]);
            if a1 - a0 < n || b1 - b0 < n {
                return Vec::new();
            }
            let mut ca: FxHashMap<u64, (u32, usize)> = FxHashMap::default();
            for i in a0..=a1 - n {
                let e = ca.entry(key(a, i)).or_insert((0, i));
                e.0 += 1;
            }
            let mut cb: FxHashMap<u64, (u32, usize)> = FxHashMap::default();
            for j in b0..=b1 - n {
                let x = key(b, j);
                if ca.get(&x).is_some_and(|e| e.0 == 1) {
                    let e = cb.entry(x).or_insert((0, j));
                    e.0 += 1;
                }
            }
            (a0..=a1 - n)
                .filter_map(|i| {
                    let x = key(a, i);
                    (ca[&x].0 == 1)
                        .then(|| cb.get(&x).filter(|e| e.0 == 1).map(|e| (i, e.1)))
                        .flatten()
                })
                .collect()
        };
        let mut uniq = unique(1);
        if uniq.is_empty() {
            uniq = unique(3);
        }
        if !uniq.is_empty() && depth < 64 {
            uniq = lis(&uniq);
            let (mut pa, mut pb) = (a0, b0);
            for (k, &(i, j)) in uniq.iter().enumerate() {
                diff(a, b, pa, i, pb, j, out, depth + 1, lead && k == 0, false);
                out.push((i, j));
                pa = i + 1;
                pb = j + 1;
            }
            diff(a, b, pa, a1, pb, b1, out, depth + 1, false, trail);
        } else {
            // (the alignment keeping the page's text together in the
            // sources; a page's end is free to stop anywhere: from the
            // end where the range is the page's end and anchored before)
            let rev = trail && !lead;
            let (sa, sb): (Vec<u64>, Vec<u64>) = if rev {
                (
                    a[a0..a1].iter().rev().copied().collect(),
                    b[b0..b1].iter().rev().copied().collect(),
                )
            } else {
                (a[a0..a1].to_vec(), b[b0..b1].to_vec())
            };
            let mut got = Vec::new();
            if sa.len() * sb.len() <= 1 << 20 {
                // (anchored: where the range follows an anchor)
                let (start, end) = if rev { (true, false) } else { (!lead, !trail) };
                got = align(&sa, &sb, start, end);
            } else {
                greedy(&sa, &sb, 0, sa.len(), 0, sb.len(), &mut got);
            }
            if rev {
                got.reverse();
                out.extend(got.into_iter().map(|(i, j)| (a1 - 1 - i, b1 - 1 - j)));
            } else {
                out.extend(got.into_iter().map(|(i, j)| (a0 + i, b0 + j)));
            }
        }
    }
    out.extend(suffix.into_iter().rev());
}

/// The alignment of `a` (a page's words) with `b` (the sources') that
/// matches the most words, a skip of `b`'s words between two matches
/// costing half a match (the page's text kept together in the sources:
/// of two copies of a sentence, the one next to the rest); `start`,
/// `end`: the range follows, or is followed by, an anchor (a skip of
/// `b` there costs too).
fn align(a: &[u64], b: &[u64], start: bool, end: bool) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let w = m + 1;
    // (f[s][i * w + j]: the best score from (i, j) in state s: 0 none
    // matched yet, 1 after a match, 2 skipping the sources after one)
    let mut f = [
        vec![0i32; (n + 1) * w],
        vec![0i32; (n + 1) * w],
        vec![0i32; (n + 1) * w],
    ];
    for i in (0..=n).rev() {
        for j in (0..=m).rev() {
            let k = i * w + j;
            if i == n {
                f[0][k] = 0;
                f[1][k] = if end && j < m { -1 } else { 0 };
                f[2][k] = 0;
                continue;
            }
            if j == m {
                f[0][k] = 0;
                f[1][k] = 0;
                f[2][k] = 0;
                continue;
            }
            let mat = if a[i] == b[j] {
                2 + f[1][k + w + 1]
            } else {
                i32::MIN
            };
            for st in 0..3 {
                let skip_a = f[st][k + w];
                let skip_b = match st {
                    0 => f[0][k + 1],
                    1 => f[2][k + 1] - 1,
                    _ => f[2][k + 1],
                };
                f[st][k] = mat.max(skip_b).max(skip_a);
            }
        }
    }
    let (mut i, mut j, mut st) = (0, 0, usize::from(start));
    let mut out = Vec::new();
    while i < n && j < m {
        let k = i * w + j;
        let here = f[st][k];
        if a[i] == b[j] && here == 2 + f[1][k + w + 1] {
            out.push((i, j));
            i += 1;
            j += 1;
            st = 1;
            continue;
        }
        let (skip_b, next) = match st {
            0 => (f[0][k + 1], 0),
            1 => (f[2][k + 1] - 1, 2),
            _ => (f[2][k + 1], 2),
        };
        if here == skip_b {
            j += 1;
            st = next;
        } else {
            i += 1;
        }
    }
    out
}

/// `a[a0..a1]` against `b[b0..b1]` in order, where neither has a word
/// once only (a text repeating itself): each of `a` matched to the next
/// equal word of `b` within 64, where the word after agrees too.
fn greedy(
    a: &[u64],
    b: &[u64],
    a0: usize,
    a1: usize,
    b0: usize,
    b1: usize,
    out: &mut Vec<(usize, usize)>,
) {
    let (mut i, mut j) = (a0, b0);
    while i < a1 && j < b1 {
        if a[i] == b[j] {
            out.push((i, j));
            i += 1;
            j += 1;
            continue;
        }
        let next = (j + 1..(j + 64).min(b1))
            .find(|&k| b[k] == a[i] && (i + 1 >= a1 || k + 1 >= b1 || a[i + 1] == b[k + 1]));
        match next {
            Some(k) => j = k,
            None => i += 1,
        }
    }
}

/// The longest run of `xs` (by `.0`, in order) whose `.1` increase.
fn lis(xs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut tails: Vec<usize> = Vec::new();
    let mut prev = vec![usize::MAX; xs.len()];
    for (k, x) in xs.iter().enumerate() {
        let p = tails.partition_point(|&t| xs[t].1 < x.1);
        if p > 0 {
            prev[k] = tails[p - 1];
        }
        if p == tails.len() {
            tails.push(k);
        } else {
            tails[p] = k;
        }
    }
    let mut out = Vec::new();
    let mut k = tails.last().copied().unwrap_or(usize::MAX);
    while k != usize::MAX {
        out.push(xs[k]);
        k = prev[k];
    }
    out.reverse();
    out
}

/// A longest common subsequence of `a` and `b`: its pairs, in order.
fn lcs(a: &[u64], b: &[u64]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut t = vec![0u16; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i * (m + 1) + j] = if a[i] == b[j] {
                t[(i + 1) * (m + 1) + j + 1] + 1
            } else {
                t[(i + 1) * (m + 1) + j].max(t[i * (m + 1) + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if t[(i + 1) * (m + 1) + j] >= t[i * (m + 1) + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

// ---- placing ----

/// A text's string offsets as the file's bytes: a Latin-1 text's
/// characters are the file's bytes (one each), any other text's own
/// UTF-8 bytes are.
#[derive(Debug, Default)]
pub(crate) struct Bytes {
    /// The string offsets of the characters past ASCII (Latin-1 texts).
    wide: Vec<u32>,
}

impl Bytes {
    pub(crate) fn of(t: &str) -> Bytes {
        if t.is_ascii() || t.chars().any(|c| u32::from(c) > 0xFF) {
            return Bytes::default();
        }
        Bytes {
            wide: t
                .char_indices()
                .filter(|x| !x.1.is_ascii())
                .map(|x| u32::try_from(x.0).unwrap_or(u32::MAX))
                .collect(),
        }
    }

    /// String offset `o` as the file's byte.
    pub(crate) fn at(&self, o: u32) -> u32 {
        o - u32::try_from(self.wide.partition_point(|&w| w < o)).unwrap_or(0)
    }
}

/// The line starts of `t`.
pub(crate) fn line_starts(t: &str) -> Vec<u32> {
    let mut v = vec![0u32];
    v.extend(
        t.bytes()
            .enumerate()
            .filter(|x| x.1 == b'\n')
            .map(|x| u32::try_from(x.0 + 1).unwrap_or(u32::MAX)),
    );
    v
}

/// A page's answer: per code of the page (in `pdftext::page_codes`'
/// order), its file (an index into [`crate::Build::origin_files`],
/// `u32::MAX` none), its byte range in the Build's version of the file,
/// whether it is synthesized (one bit a code), the glyph's origin in
/// PDF points from the page's top left, its width and its font's size
/// (points; 0 for a code no character made).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageOrigins {
    pub file: Vec<u32>,
    pub start: Vec<u32>,
    pub end: Vec<u32>,
    pub synth: Vec<u32>,
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub w: Vec<f32>,
    pub size: Vec<f32>,
}

impl PageOrigins {
    /// How many codes the page shows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.file.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.file.is_empty()
    }

    /// Whether code `i` is synthesized (a macro's glyph: the call's range).
    #[must_use]
    pub fn is_synth(&self, i: usize) -> bool {
        self.synth
            .get(i / 32)
            .is_some_and(|w| w >> (i % 32) & 1 == 1)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push(
        &mut self,
        file: u32,
        start: u32,
        end: u32,
        synth: bool,
        x: f32,
        y: f32,
        w: f32,
        size: f32,
    ) {
        let i = self.file.len();
        if i.is_multiple_of(32) {
            self.synth.push(0);
        }
        if synth && let Some(w) = self.synth.last_mut() {
            *w |= 1 << (i % 32);
        }
        self.file.push(file);
        self.start.push(start);
        self.end.push(end);
        self.x.push(x);
        self.y.push(y);
        self.w.push(w);
        self.size.push(size);
    }
}

/// Every page's origins, the files they name and their texts
/// ([`crate::Build::origins_snapshot`]).
#[derive(Clone, Debug, Default)]
pub struct OriginsSnapshot {
    pub pages: Vec<Option<Arc<PageOrigins>>>,
    pub files: Vec<String>,
    pub texts: Vec<Option<Arc<[u8]>>>,
}

/// sp to PDF points.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
pub(crate) fn bp(s: Scaled) -> f32 {
    (f64::from(s) / 65536.0 * 72.0 / 72.27) as f32
}

// ---- the Build's side ----

/// What origins are made from ([`crate::Build::origins_input`]).
pub(crate) struct Input<'a> {
    /// The value store's instance.
    pub inst: u64,
    /// The pages' identities in the link, in the file's order.
    pub shown: Vec<u64>,
    /// The text nodes: raw id, file (none: the document) and its text,
    /// byte range, the document node it is under.
    #[allow(clippy::type_complexity)]
    pub spans: Vec<(u64, Option<(String, Arc<str>)>, usize, usize, usize)>,
    /// The document's name and the text its nodes hold (its canonical
    /// text where `doc_raw` is the text given).
    pub doc_name: String,
    pub doc_text: &'a str,
    pub doc_raw: Option<&'a str>,
}

/// A text node as placed: its file (index into the files) and start.
#[derive(Clone, Copy)]
struct Span {
    file: u32,
    s: u32,
    len: u32,
    /// The document's own text (its offsets go over to the raw text).
    doc: bool,
    /// The hash of its text.
    hash: u64,
}

/// What an answer is made with: the index, the spans by raw id, each
/// file's text (as held, and how offsets in it become the file's bytes).
struct Ctx<'a> {
    inp: Input<'a>,
    index: Arc<Index>,
    spans: FxHashMap<u64, Span>,
    /// Per file (by index): its text as the nodes hold it.
    texts: Vec<Option<Arc<str>>>,
    bytes: Mutex<FxHashMap<u32, Arc<Bytes>>>,
    canon: OnceLock<(Vec<u32>, Vec<u32>)>,
}

/// The Build's origins memos: the nodes' scans, the index, the pages'
/// matches, the files named (append-only), the last answers given.
#[derive(Default)]
pub(crate) struct State {
    scans: Mutex<FxHashMap<u64, Arc<NodeScan>>>,
    index: Mutex<Option<(u64, Arc<Index>)>>,
    matches: Mutex<FxHashMap<u64, Arc<Matched>>>,
    /// The pages' answers, by page and what they were placed from.
    answers: Mutex<FxHashMap<u64, (u64, Arc<PageOrigins>)>>,
    files: Mutex<Vec<String>>,
    last: Mutex<Last>,
    /// The PDF a DVI or XDV build's file became, as its client made it
    /// ([`crate::Build::set_origins_pdf`]): its forms' codes.
    pdf: Mutex<Option<Arc<Vec<u8>>>>,
}

/// The answers [`crate::Build::origins_changed`] gave last, and the
/// files' texts then (as bytes).
#[derive(Default)]
struct Last {
    pages: Vec<Option<Arc<PageOrigins>>>,
    texts: FxHashMap<u32, Arc<[u8]>>,
}

/// A text as the file's bytes (Latin-1: a character a byte).
fn file_bytes(t: &str) -> Arc<[u8]> {
    if t.is_ascii() || t.chars().any(|c| u32::from(c) > 0xFF) {
        return Arc::from(t.as_bytes());
    }
    t.chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
        .collect()
}

impl State {
    fn file_index(&self, name: &str) -> u32 {
        let mut f = self.files.locked();
        let i = f.iter().position(|n| n == name).unwrap_or_else(|| {
            f.push(name.to_owned());
            f.len() - 1
        });
        u32::try_from(i).unwrap_or(u32::MAX)
    }

    fn ctx<'a>(&self, inp: Input<'a>) -> Ctx<'a> {
        let doc = self.file_index(&inp.doc_name);
        let mut texts: Vec<Option<Arc<str>>> = Vec::new();
        let mut spans: FxHashMap<u64, Span> = FxHashMap::default();
        // (the nodes in the document's order; a node inside another of
        // its file left out: its text is the other's)
        let mut order: Vec<usize> = (0..inp.spans.len()).collect();
        order.sort_by(|&i, &j| {
            let (x, y) = (&inp.spans[i], &inp.spans[j]);
            (
                x.4,
                x.1.is_some(),
                x.1.as_ref().map(|f| f.0.as_str()),
                x.2,
                std::cmp::Reverse(x.3),
            )
                .cmp(&(
                    y.4,
                    y.1.is_some(),
                    y.1.as_ref().map(|f| f.0.as_str()),
                    y.2,
                    std::cmp::Reverse(y.3),
                ))
        });
        let mut nodes: Vec<Node> = Vec::new();
        let memo = self.scans.locked();
        let mut fresh: FxHashMap<u64, Arc<NodeScan>> = FxHashMap::default();
        let mut reach: Option<(Option<String>, usize)> = None;
        for &i in &order {
            let (raw, f, s, e, _) = &inp.spans[i];
            let name = f.as_ref().map(|x| x.0.clone());
            if let Some((n, end)) = &reach
                && *n == name
                && *e <= *end
                && f.is_some()
            {
                continue;
            }
            reach = Some((name.clone(), *e));
            let (fi, text): (u32, &str) = match f {
                None => (doc, inp.doc_text),
                Some((n, t)) => (self.file_index(n.trim_start_matches("./")), t),
            };
            let fu = fi as usize;
            if texts.len() <= fu {
                texts.resize(fu + 1, None);
            }
            if texts[fu].is_none() {
                texts[fu] = Some(match f {
                    None => Arc::from(inp.doc_raw.unwrap_or(inp.doc_text)),
                    Some((_, t)) => t.clone(),
                });
            }
            let t = text.get(*s..(*e).min(text.len())).unwrap_or("");
            let h = hash_bytes(0x6f72_6967, t.as_bytes());
            let sc = match memo.get(raw) {
                Some(x) if x.text_hash == h => x.clone(),
                _ => {
                    let x = Arc::new(scan(t));
                    fresh.insert(*raw, x.clone());
                    x
                }
            };
            spans.insert(
                *raw,
                Span {
                    file: fi,
                    s: u32::try_from(*s).unwrap_or(u32::MAX),
                    len: u32::try_from(t.len()).unwrap_or(u32::MAX),
                    doc: f.is_none(),
                    hash: sc.text_hash,
                },
            );
            nodes.push(Node {
                raw: *raw,
                scan: sc,
            });
        }
        drop(memo);
        if !fresh.is_empty() {
            let mut memo = self.scans.locked();
            memo.extend(fresh);
            // (the nodes gone: dropped)
            memo.retain(|k, _| spans.contains_key(k));
        }
        let key = nodes.iter().fold(0x696e_6478_u64, |h, n| {
            hash_words(h, &[n.raw, n.scan.text_hash])
        });
        let index = {
            let g = self.index.locked();
            g.as_ref().filter(|x| x.0 == key).map(|x| x.1.clone())
        };
        let index = index.unwrap_or_else(|| {
            let ix = Arc::new(Index::new(nodes));
            *self.index.locked() = Some((key, ix.clone()));
            ix
        });
        // (the document's own text, as given: where the nodes hold its
        // canonical text, offsets go over by line and column)
        Ctx {
            inp,
            index,
            spans,
            texts,
            bytes: Mutex::new(FxHashMap::default()),
            canon: OnceLock::new(),
        }
    }

    /// Page `k`'s match, kept while its glyphs are the same and every
    /// node it names is there with the same text.
    fn matched(&self, cx: &Ctx<'_>, id: u64, g: &PageGlyphs) -> Arc<Matched> {
        let kept = self.matches.locked().get(&id).cloned();
        if let Some(m) = kept
            && m.nodes
                .iter()
                .all(|&(raw, h)| cx.spans.get(&raw).is_some_and(|sp| sp.hash == h))
        {
            return m;
        }
        let m = Arc::new(match_page(g, &cx.index));
        let mut all = self.matches.locked();
        if all.len() > 1 << 16 {
            all.clear();
        }
        all.insert(id, m.clone());
        m
    }
}

impl Ctx<'_> {
    fn bytes(&self, file: u32) -> Arc<Bytes> {
        if let Some(b) = self.bytes.locked().get(&file) {
            return b.clone();
        }
        let b = Arc::new(
            self.texts
                .get(file as usize)
                .and_then(Option::as_ref)
                .map_or_else(Bytes::default, |t| Bytes::of(t)),
        );
        self.bytes.locked().insert(file, b.clone());
        b
    }

    /// Offset `o` of the text the nodes hold for `file` as the file's
    /// byte.
    fn place(&self, file: u32, doc: bool, o: u32) -> u32 {
        let o = match (doc, self.inp.doc_raw) {
            (true, Some(raw)) => {
                let (c, r) = self
                    .canon
                    .get_or_init(|| (line_starts(self.inp.doc_text), line_starts(raw)));
                let l = c.partition_point(|&x| x <= o).saturating_sub(1);
                let col = o - c[l];
                let end = r
                    .get(l + 1)
                    .map_or(u32::try_from(raw.len()).unwrap_or(u32::MAX), |&x| {
                        x.saturating_sub(1)
                    });
                (r.get(l).copied().unwrap_or(0) + col).min(end)
            }
            _ => o,
        };
        self.bytes(file).at(o)
    }
}

/// Code counts of each form or included page page `k` (0-based) of PDF
/// `pdf` draws, in its content stream's order.
fn xobject_counts(pdf: &[u8], k: usize) -> Vec<usize> {
    use phitex_codecs::pdftext::{DocText, Font, State as TState, Text};
    struct Per<'a> {
        inner: DocText<'a>,
        counts: Vec<usize>,
    }
    impl Text for Per<'_> {
        fn font(&mut self, name: &[u8]) -> Font {
            self.inner.font(name)
        }
        fn code(&mut self, font: &[u8], code: u32, x: f64, y: f64) {
            self.inner.code(font, code, x, y);
        }
        fn draw(&mut self, name: &[u8], state: &TState) {
            let n = self.inner.out.len();
            self.inner.draw(name, state);
            self.counts.push(self.inner.out.len() - n);
        }
    }
    let data: Arc<[u8]> = Arc::from(pdf);
    let Ok(doc) = phitex_codecs::pdfread::Doc::open(&data) else {
        return Vec::new();
    };
    let Some(page) = doc.page(k + 1) else {
        return Vec::new();
    };
    let content = phitex_codecs::pdftext::page_content(&doc, &page);
    let mut out = Vec::new();
    let mut t = Per {
        inner: DocText {
            doc: &doc,
            resources: page.resources.clone(),
            depth: 0,
            out: &mut out,
        },
        counts: Vec::new(),
    };
    phitex_codecs::pdftext::walk(&content, &mut t);
    t.counts
}

/// Page `k`'s answer.
fn answer(
    st: &State,
    cx: &Ctx<'_>,
    k: usize,
    pdf: &dyn Fn() -> Arc<Vec<u8>>,
) -> Option<Arc<PageOrigins>> {
    let id = *cx.inp.shown.get(k)?;
    let g = kept(cx.inp.inst, id)?;
    let m = st.matched(cx, id, &g);
    let nodes: Vec<Option<(Span, bool)>> = m
        .nodes
        .iter()
        .map(|(raw, _)| cx.spans.get(raw).map(|&sp| (sp, sp.doc)))
        .collect();
    // (the answer kept while its match is the same and its nodes are
    // placed where they were: their ends in their files)
    let key = nodes.iter().fold(
        hash_words(id, &[Arc::as_ptr(&m) as usize as u64]),
        |h, n| match n {
            Some((sp, doc)) => hash_words(
                h,
                &[
                    u64::from(sp.file),
                    u64::from(cx.place(sp.file, *doc, sp.s)),
                    u64::from(cx.place(sp.file, *doc, sp.s + sp.len)),
                ],
            ),
            None => hash_words(h, &[u64::MAX]),
        },
    );
    if let Some((k0, a)) = st.answers.locked().get(&id)
        && *k0 == key
    {
        return Some(a.clone());
    }
    let counts = if g.codes.iter().any(|c| c.kind == XOBJECT) {
        xobject_counts(&pdf(), k)
    } else {
        Vec::new()
    };
    let mut out = PageOrigins::default();
    let (mut x, mut y) = (0.0, 0.0);
    let mut forms = counts.iter();
    for (c, o) in g.codes.iter().zip(&m.orgs) {
        match c.kind {
            GLYPH => {
                (x, y) = (bp(c.h), bp(c.v));
                let (w, z) = (bp(c.w), bp(c.size));
                match nodes.get(o.node as usize).copied().flatten() {
                    Some((sp, doc)) => {
                        let a = cx.place(sp.file, doc, sp.s + o.from);
                        let b = cx.place(sp.file, doc, sp.s + o.to);
                        out.push(sp.file, a, b, o.synth, x, y, w, z);
                    }
                    None => out.push(u32::MAX, 0, 0, false, x, y, w, z),
                }
            }
            XOBJECT => {
                for _ in 0..forms.next().copied().unwrap_or(0) {
                    out.push(u32::MAX, 0, 0, false, x, y, 0.0, 0.0);
                }
            }
            _ => out.push(u32::MAX, 0, 0, false, x, y, 0.0, 0.0),
        }
    }
    let out = Arc::new(out);
    let mut all = st.answers.locked();
    if all.len() > 1 << 16 {
        all.clear();
    }
    all.insert(id, (key, out.clone()));
    Some(out)
}

impl crate::Build {
    /// The PDF a DVI or XDV file became (dvipdfmx, xdvipdfmx), made by
    /// the client: where a page draws a picture or a form, the codes it
    /// shows are counted in it. A PDF build's file is its own.
    pub fn set_origins_pdf(&self, pdf: Arc<Vec<u8>>) {
        *self.origins_state().pdf.locked() = Some(pdf);
    }

    /// The PDF the pages' codes are counted in: the client's
    /// ([`Self::set_origins_pdf`]), or the build's file.
    fn origins_pdf(&self) -> Arc<Vec<u8>> {
        if let Some(p) = self.origins_state().pdf.locked().clone() {
            return p;
        }
        self.output_bytes()
    }

    /// Page `k`'s glyph origins (0 the first; docs/note-origins.md): per
    /// code the page shows, in `pdftext::page_codes`' order, the file
    /// ([`Self::origin_files`]) and byte range it came from, in the
    /// Build's version of the file. `None`: origins off
    /// ([`crate::Options::origins`]), no page `k`, or a page whose
    /// writer kept none. Callable from any thread while the Build is
    /// idle: it takes no lock an evaluation holds.
    #[must_use]
    pub fn page_origins(&self, k: usize) -> Option<Arc<PageOrigins>> {
        let st = self.origins_state();
        let cx = st.ctx(self.origins_input());
        answer(st, &cx, k, &|| self.origins_pdf())
    }

    /// Every page's origins ([`Self::page_origins`]), made with one
    /// look at the sources.
    #[must_use]
    pub fn all_page_origins(&self) -> Vec<Option<Arc<PageOrigins>>> {
        let st = self.origins_state();
        let cx = st.ctx(self.origins_input());
        let pdf: OnceLock<Arc<Vec<u8>>> = OnceLock::new();
        let get = || pdf.get_or_init(|| self.origins_pdf()).clone();
        (0..cx.inp.shown.len())
            .map(|k| answer(st, &cx, k, &get))
            .collect()
    }

    /// The texts of the files origins name ([`Self::origin_files`], by
    /// index), as the Build holds them: the bytes the offsets are in.
    #[must_use]
    pub fn origin_texts(&self) -> Vec<Option<Arc<[u8]>>> {
        self.origins_snapshot().texts
    }

    /// Every page's origins, the files they name and those files' texts
    /// ([`Self::all_page_origins`], [`Self::origin_files`],
    /// [`Self::origin_texts`]), made with one look at the sources.
    #[must_use]
    pub fn origins_snapshot(&self) -> OriginsSnapshot {
        let t0 = std::time::Instant::now();
        let st = self.origins_state();
        let inp = self.origins_input();
        let t1 = t0.elapsed();
        let cx = st.ctx(inp);
        let t2 = t0.elapsed();
        let pdf: OnceLock<Arc<Vec<u8>>> = OnceLock::new();
        let get = || pdf.get_or_init(|| self.origins_pdf()).clone();
        let pages = (0..cx.inp.shown.len())
            .map(|k| answer(st, &cx, k, &get))
            .collect();
        let t3 = t0.elapsed();
        let files = st.files.locked().clone();
        let texts = (0..files.len())
            .map(|i| {
                cx.texts
                    .get(i)
                    .and_then(Option::as_ref)
                    .map(|t| file_bytes(t))
            })
            .collect();
        if crate::exec::env_flag!("PHITEX_ORIGINS_TIME") {
            eprintln!(
                "origins: input {t1:?} (spans {}), sources {:?} (nodes {}), pages {:?}, texts {:?}",
                cx.inp.spans.len(),
                t2.saturating_sub(t1),
                cx.index.nodes.len(),
                t3.saturating_sub(t2),
                t0.elapsed().saturating_sub(t3)
            );
        }
        OriginsSnapshot {
            pages,
            files,
            texts,
        }
    }

    /// The files origins name, by their index in [`PageOrigins::file`]:
    /// append-only, an index means the same file for the Build's life.
    #[must_use]
    pub fn origin_files(&self) -> Vec<String> {
        self.origins_state().files.locked().clone()
    }

    /// The pages whose origins changed since the last call (all of them
    /// the first time), and the edits of the files they name since then:
    /// `(file, at, delta)`, a byte at or past `at` (of the file as it
    /// was) now `delta` further, a page's offsets not in the list moved
    /// by them alone.
    pub fn origins_changed(&self) -> (Vec<usize>, Vec<(u32, u32, i64)>) {
        let st = self.origins_state();
        let pages = self.all_page_origins();
        let cx_texts: Vec<(u32, Arc<[u8]>)> = {
            let inp = self.origins_input();
            let mut ts: Vec<(u32, Arc<[u8]>)> = vec![(
                st.file_index(&inp.doc_name),
                file_bytes(inp.doc_raw.unwrap_or(inp.doc_text)),
            )];
            let mut seen: phitex_defuse::FxHashSet<String> = phitex_defuse::FxHashSet::default();
            for (_, f, ..) in &inp.spans {
                if let Some((n, t)) = f
                    && seen.insert(n.clone())
                {
                    ts.push((st.file_index(n.trim_start_matches("./")), file_bytes(t)));
                }
            }
            ts
        };
        let mut last = st.last.locked();
        let mut deltas: Vec<(u32, u32, i64)> = Vec::new();
        for (f, t) in &cx_texts {
            if let Some(old) = last.texts.get(f)
                && **old != **t
            {
                let p = old.iter().zip(t.iter()).take_while(|(a, b)| a == b).count();
                let max = old.len().min(t.len()) - p;
                let q = old
                    .iter()
                    .rev()
                    .zip(t.iter().rev())
                    .take(max)
                    .take_while(|(a, b)| a == b)
                    .count();
                let at = u32::try_from(old.len() - q).unwrap_or(u32::MAX);
                let d = i64::try_from(t.len()).unwrap_or(0) - i64::try_from(old.len()).unwrap_or(0);
                if d != 0 {
                    deltas.push((*f, at, d));
                }
            }
        }
        let shift = |f: u32, o: u32| -> u32 {
            deltas
                .iter()
                .filter(|d| d.0 == f && o >= d.1)
                .fold(i64::from(o), |o, d| o + d.2)
                .try_into()
                .unwrap_or(u32::MAX)
        };
        let mut changed = Vec::new();
        for (k, p) in pages.iter().enumerate() {
            let same = match (last.pages.get(k).cloned().flatten(), p) {
                (Some(a), Some(b)) if Arc::ptr_eq(&a, b) && deltas.is_empty() => true,
                (Some(a), Some(b)) => {
                    a.len() == b.len()
                        && a.synth == b.synth
                        && a.x == b.x
                        && a.y == b.y
                        && (0..a.len()).all(|i| {
                            a.file[i] == b.file[i]
                                && (a.file[i] == u32::MAX
                                    || (shift(a.file[i], a.start[i]) == b.start[i]
                                        && shift(a.file[i], a.end[i]) == b.end[i]))
                        })
                }
                (None, None) => last.pages.len() > k,
                _ => false,
            };
            if !same {
                changed.push(k);
            }
        }
        last.pages = pages;
        last.texts = cx_texts.into_iter().collect();
        (changed, deltas)
    }

    /// The glyph with a source nearest point (`x`, `y`) of page `k` (PDF
    /// points from its top left; a line apart weighs more than a word
    /// apart): its file, byte range and whether it is synthesized.
    #[must_use]
    pub fn nearest(&self, k: usize, x: f32, y: f32) -> Option<(String, u32, u32, bool)> {
        let p = self.page_origins(k)?;
        let mut best: Option<(f32, usize)> = None;
        for i in 0..p.len() {
            if p.file[i] == u32::MAX {
                continue;
            }
            let (dx, dy) = (p.x[i] + 2.5 - x, p.y[i] - 3.0 - y);
            let e = dx * dx + 9.0 * dy * dy + if p.is_synth(i) { 400.0 } else { 0.0 };
            if best.is_none_or(|b| e < b.0) {
                best = Some((e, i));
            }
        }
        let (_, i) = best?;
        let files = self.origin_files();
        Some((
            files.get(p.file[i] as usize)?.clone(),
            p.start[i],
            p.end[i],
            p.is_synth(i),
        ))
    }

    /// The glyphs that came from bytes `lo..hi` of `file` (not
    /// synthesized): `(page, code)` each.
    #[must_use]
    pub fn glyphs_of(&self, file: &str, lo: u32, hi: u32) -> Vec<(usize, usize)> {
        let Some(f) = self
            .origin_files()
            .iter()
            .position(|n| n == file.trim_start_matches("./"))
        else {
            return Vec::new();
        };
        let f = u32::try_from(f).unwrap_or(u32::MAX);
        let mut out = Vec::new();
        for (k, p) in self.all_page_origins().iter().enumerate() {
            let Some(p) = p else { continue };
            for i in 0..p.len() {
                if p.file[i] == f && !p.is_synth(i) && p.start[i] < hi && p.end[i] > lo {
                    out.push((k, i));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_keeps_text_and_skips_commands() {
        let s = scan("A \\emph{word} and caf\\'e % note\n\\LaTeX\\ is 100\\%.");
        let words: Vec<String> = s
            .words
            .iter()
            .map(|w| {
                s.cands[w.0 as usize..w.1 as usize]
                    .iter()
                    .map(|c| c.0)
                    .collect()
            })
            .collect();
        assert_eq!(words, ["A", "word", "and", "cafe", "is", "100%."]);
        assert_eq!(s.calls.len(), 4);
    }

    #[test]
    fn diff_matches_in_order() {
        let a = [1, 2, 3, 9, 4, 5];
        let b = [7, 1, 2, 3, 4, 8, 5];
        let mut out = Vec::new();
        diff(&a, &b, 0, a.len(), 0, b.len(), &mut out, 0, false, false);
        assert_eq!(out, [(0, 1), (1, 2), (2, 3), (4, 4), (5, 6)]);
    }

    #[test]
    fn latin1_offsets_are_bytes() {
        let t: String = [b'a', 0xE9, b'b'].iter().map(|&b| char::from(b)).collect();
        let b = Bytes::of(&t);
        assert_eq!(b.at(3), 2);
        assert_eq!(b.at(1), 1);
    }
}
