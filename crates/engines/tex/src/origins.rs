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
/// A form or an included page drawn ([`Code::lig`]: its object), its
/// codes as the writer told them ([`PageGlyphs::xcodes`]).
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
    /// Its reversal level ([`phitex_pdf::GlyphTap::level`]: odd in
    /// right-to-left text, which the page shows in the reverse of its
    /// reading order) and the context it is in (the codes between two
    /// returns to level 0 are read again in their order together).
    pub level: u8,
    pub context: u32,
}

/// A page's codes as its writer showed them.
#[derive(Clone, Debug, Default)]
pub(crate) struct PageGlyphs {
    pub codes: Vec<Code>,
    pub lig: Vec<u16>,
    /// The codes each object drawn shows ([`XOBJECT`]): its object and
    /// count, as the writer told them.
    pub xcodes: Vec<(u32, usize)>,
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
    /// The reversal level now, and its context ([`Code::level`]).
    level: u8,
    context: u32,
}

impl Recorder {
    pub(crate) fn new() -> (Recorder, Arc<Mutex<Option<PageGlyphs>>>) {
        let out = Arc::new(Mutex::new(None));
        (
            Recorder {
                g: PageGlyphs::default(),
                out: out.clone(),
                level: 0,
                context: 0,
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
            level: self.level,
            context: self.context,
        });
    }

    fn level(&mut self, level: u8) {
        if self.level == 0 && level > 0 {
            self.context += 1;
        }
        self.level = level;
    }

    fn none(&mut self, n: usize) {
        for _ in 0..n {
            self.g.codes.push(Code {
                kind: NONE,
                level: self.level,
                context: self.context,
                ..Code::default()
            });
        }
    }

    fn literal(&mut self, bytes: &[u8]) {
        self.none(phitex_pdf::literal_codes(bytes));
    }

    fn xobject(&mut self, obj: usize) {
        self.g.codes.push(Code {
            kind: XOBJECT,
            lig: u32::try_from(obj).unwrap_or(u32::MAX),
            level: self.level,
            context: self.context,
            ..Code::default()
        });
    }

    fn xobject_codes(&mut self, obj: usize, codes: usize) {
        let obj = u32::try_from(obj).unwrap_or(u32::MAX);
        if !self.g.xcodes.iter().any(|&(o, _)| o == obj) {
            self.g.xcodes.push((obj, codes));
        }
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
                    level: self.level,
                    context: self.context,
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
                level: self.level,
                context: self.context,
            });
        }
    }
}

/// The pages' glyphs by the value store and the page's identity in the
/// link (`crate::pdfout::Linked::shown`), and whether the link's own
/// writer showed them ([`keep_shipped`]).
type GlyphMemo = Mutex<FxHashMap<(u64, u64), (Arc<PageGlyphs>, bool)>>;

fn glyph_memo() -> &'static GlyphMemo {
    static M: OnceLock<GlyphMemo> = OnceLock::new();
    M.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// The pieces (their nodes' raw ids) whose sealed lines a page shows,
/// by the value store and the page's identity in the link: where a
/// source place is on the pages, with no page matched
/// ([`crate::Build::pages_of_source`]).
type PieceMemo = Mutex<FxHashMap<(u64, u64), Arc<[i64]>>>;

fn piece_memo() -> &'static PieceMemo {
    static M: OnceLock<PieceMemo> = OnceLock::new();
    M.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// Page `id`'s pieces (value store `inst`), kept.
pub(crate) fn keep_pieces(inst: u64, id: u64, pieces: &[i64]) {
    let mut m = piece_memo().locked();
    if m.get(&(inst, id)).is_some_and(|p| **p == *pieces) {
        return;
    }
    if m.len() > 1 << 16 {
        m.clear();
    }
    m.insert((inst, id), Arc::from(pieces));
}

/// Page `id`'s glyphs (value store `inst`), kept, as the writer the page
/// was written apart on showed them: unless the link's own are kept.
pub(crate) fn keep(inst: u64, id: u64, g: PageGlyphs) {
    put(inst, id, g, false);
}

/// Page `id`'s glyphs (value store `inst`) as the link's writer showed
/// them, where it shipped the page again: kept over the page's own
/// (the forms and images the page draws are the link's, not known to
/// the writer it was written apart on).
pub(crate) fn keep_shipped(inst: u64, id: u64, g: PageGlyphs) {
    put(inst, id, g, true);
}

fn put(inst: u64, id: u64, g: PageGlyphs, shipped: bool) {
    let mut m = glyph_memo().locked();
    if !shipped && m.get(&(inst, id)).is_some_and(|e| e.1) {
        return;
    }
    // (the pages of builds gone: dropped wholesale past a bound, made
    // again where a page's writer runs)
    if m.len() > 1 << 16 {
        m.clear();
    }
    m.insert((inst, id), (Arc::new(g), shipped));
}

/// Page `id`'s glyphs (value store `inst`) dropped: its record is gone.
pub(crate) fn forget(inst: u64, id: u64) {
    glyph_memo().locked().remove(&(inst, id));
}

/// Page `id`'s glyphs (value store `inst`), if kept.
pub(crate) fn kept(inst: u64, id: u64) -> Option<Arc<PageGlyphs>> {
    glyph_memo().locked().get(&(inst, id)).map(|e| e.0.clone())
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
/// to the nodes that have them (more than [`CAP`]: common, never a
/// vote). The postings are by node (its raw id), kept from one answer to
/// the next ([`Postings`]): an edit takes the nodes it changed out and
/// puts them in again, the others' entries stay; `nodes` are the nodes
/// in the document's order, `order` each one's place there.
#[derive(Default)]
pub(crate) struct Index {
    pub nodes: Vec<Node>,
    order: FxHashMap<u64, u32>,
    post: Arc<Postings>,
}

/// Each shingle's nodes and each long word's (distinct, by raw id), and
/// the scan each node is in by ([`Index`]).
#[derive(Default, Clone)]
pub(crate) struct Postings {
    shingles: FxHashMap<u64, Vec<u64>>,
    singles: FxHashMap<u64, Vec<u64>>,
    scans: FxHashMap<u64, Arc<NodeScan>>,
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

/// A node's shingles (four words in a row, and four of five in a row
/// with one of the middle three left out: a word no page shows, a
/// label's, between them) and its words of four characters or more,
/// each once.
fn node_keys(sc: &NodeScan) -> (Vec<u64>, Vec<u64>) {
    let hs: Vec<u64> = sc.words.iter().map(|w| w.2).collect();
    let mut sh: Vec<u64> = hs.windows(4).map(shingle).collect();
    sh.extend(hs.windows(5).flat_map(skip_one));
    sh.sort_unstable();
    sh.dedup();
    let mut si: Vec<u64> = sc
        .words
        .iter()
        .filter(|w| w.1 - w.0 >= 4)
        .map(|w| w.2)
        .collect();
    si.sort_unstable();
    si.dedup();
    (sh, si)
}

impl Postings {
    /// Node `raw` (scan `sc`) put in.
    fn put(&mut self, raw: u64, sc: &Arc<NodeScan>) {
        let (sh, si) = node_keys(sc);
        for h in sh {
            self.shingles.entry(h).or_default().push(raw);
        }
        for h in si {
            self.singles.entry(h).or_default().push(raw);
        }
        self.scans.insert(raw, sc.clone());
    }

    /// Node `raw` taken out.
    fn take(&mut self, raw: u64) {
        let Some(sc) = self.scans.remove(&raw) else {
            return;
        };
        let (sh, si) = node_keys(&sc);
        for (m, hs) in [(&mut self.shingles, sh), (&mut self.singles, si)] {
            for h in hs {
                if let Some(e) = m.get_mut(&h) {
                    e.retain(|&r| r != raw);
                    if e.is_empty() {
                        m.remove(&h);
                    }
                }
            }
        }
    }
}

impl Index {
    /// The index of `nodes` (in the document's order) from the postings
    /// `post` made last: the nodes gone or changed taken out, the new or
    /// changed put in. The postings are those [`Index`] made afresh would
    /// hold: each shingle's nodes as a set, read only where they are
    /// [`CAP`] or fewer.
    pub(crate) fn update(nodes: Vec<Node>, mut post: Arc<Postings>) -> Index {
        let order: FxHashMap<u64, u32> = nodes
            .iter()
            .enumerate()
            .map(|(k, n)| (n.raw, u32::try_from(k).unwrap_or(u32::MAX)))
            .collect();
        let stale: Vec<u64> = post
            .scans
            .iter()
            .filter(|(raw, sc)| {
                order
                    .get(raw)
                    .is_none_or(|&k| !Arc::ptr_eq(&nodes[k as usize].scan, sc))
            })
            .map(|(&raw, _)| raw)
            .collect();
        let fresh: Vec<&Node> = nodes
            .iter()
            .filter(|n| {
                post.scans
                    .get(&n.raw)
                    .is_none_or(|sc| !Arc::ptr_eq(sc, &n.scan))
            })
            .collect();
        if !stale.is_empty() || !fresh.is_empty() {
            let p = Arc::make_mut(&mut post);
            for raw in stale {
                p.take(raw);
            }
            for n in fresh {
                p.put(n.raw, &n.scan);
            }
        }
        Index { nodes, order, post }
    }

    /// The nodes (by place) shingle `h` names, where they are [`CAP`] or
    /// fewer.
    fn shingle_nodes(&self, h: u64) -> Option<impl Iterator<Item = u32> + '_> {
        self.post
            .shingles
            .get(&h)
            .filter(|ns| ns.len() <= CAP)
            .map(|ns| ns.iter().filter_map(|r| self.order.get(r).copied()))
    }

    /// The same for single word `h`, where they are two or fewer.
    fn single_nodes(&self, h: u64) -> Option<impl Iterator<Item = u32> + '_> {
        self.post
            .singles
            .get(&h)
            .filter(|ns| ns.len() <= 2)
            .map(|ns| ns.iter().filter_map(|r| self.order.get(r).copied()))
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
    /// Its reversal level ([`Code::level`]): odd, read right to left.
    level: u8,
}

/// The page's codes (indices) in their reading order: as the page shows
/// them, but for right-to-left text, which TeX--XeT's writer shows in the
/// reverse of its reading order ([`Code::level`]). Each context's codes
/// put back as the bidi algorithm's reordering (rule L2, its own inverse)
/// puts them: from its highest level down to 1, every run at that level
/// or higher reversed. A node's codes stay together, in their order (a
/// native word's glyphs, a virtual font's character's codes).
fn reading_order(codes: &[Code]) -> Vec<usize> {
    if codes.iter().all(|c| c.level == 0) {
        return (0..codes.len()).collect();
    }
    // (a node's codes: its first and its further ones)
    let mut units: Vec<(usize, usize)> = Vec::new();
    for (i, c) in codes.iter().enumerate() {
        match units.last_mut() {
            Some(u) if c.kind == GLYPH && !c.first => u.1 = i + 1,
            _ => units.push((i, i + 1)),
        }
    }
    let level = |u: &(usize, usize)| codes[u.0].level;
    let context = |u: &(usize, usize)| codes[u.0].context;
    let mut k = 0;
    while k < units.len() {
        if level(&units[k]) == 0 {
            k += 1;
            continue;
        }
        let mut e = k;
        while e < units.len() && level(&units[e]) > 0 && context(&units[e]) == context(&units[k]) {
            e += 1;
        }
        let run = &mut units[k..e];
        let top = run.iter().map(level).max().unwrap_or(0);
        for l in (1..=top).rev() {
            let mut i = 0;
            while i < run.len() {
                if level(&run[i]) < l {
                    i += 1;
                    continue;
                }
                let mut j = i;
                while j < run.len() && level(&run[j]) >= l {
                    j += 1;
                }
                run[i..j].reverse();
                i = j;
            }
        }
        k = e;
    }
    units.into_iter().flat_map(|(a, b)| a..b).collect()
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
pub(crate) fn match_page(g: &PageGlyphs, ix: &Index) -> Matched {
    match_page_until(g, ix, &|_| false).expect("never stopped")
}

/// [`match_page`], given up (`None`) once `stop` says so, between its
/// clusters' rounds and steps: a page's match is the longest step of
/// the idle work (para4000: 7 to 16 ms), which a keystroke waited for.
#[allow(clippy::too_many_lines)]
pub(crate) fn match_page_until(
    g: &PageGlyphs,
    ix: &Index,
    stop: &dyn Fn(bool) -> bool,
) -> Option<Matched> {
    // the glyphs, one a node (its first code)
    let mut glyphs: Vec<Glyph> = Vec::new();
    let mut atoms: Vec<Atom> = Vec::new();
    // (each glyph's atoms: a range)
    let mut glyph_atoms: Vec<(u32, u32)> = Vec::new();
    // (in the reading order: right-to-left text as it is read, not as the
    // page shows it, [`reading_order`])
    for i in reading_order(&g.codes) {
        let c = &g.codes[i];
        if c.kind != GLYPH {
            continue;
        }
        // (a node's further codes: its extent to theirs, a native
        // word's glyphs after its first, on either side of it)
        if !c.first {
            if let Some(gl) = glyphs.last_mut()
                && c.v == gl.v
            {
                let end = (gl.h + gl.w).max(c.h + c.w);
                gl.h = gl.h.min(c.h);
                gl.w = end - gl.h;
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
            level: c.level,
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
            // (a space between them, or a step back, in their direction:
            // right-to-left text read from its right; a change of
            // direction ends a word)
            let s = p.size.max(1);
            let gap = match (p.level % 2, gl.level % 2) {
                (0, 0) => gl.h > p.h + p.w + s / 6 || gl.h < p.h - s / 2,
                (1, 1) => gl.h + gl.w < p.h - s / 6 || gl.h + gl.w > p.h + p.w + s / 2,
                _ => true,
            };
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
        if let Some(ns) = ix.shingle_nodes(sh) {
            let ns: Vec<u32> = ns.collect();
            #[allow(clippy::cast_precision_loss)]
            let w = 1.0 / ns.len() as f64;
            for n in ns {
                *votes.entry(n).or_default() += w;
            }
        }
    }
    if votes.is_empty() {
        for &h in &whash {
            if let Some(ns) = ix.single_nodes(h) {
                let ns: Vec<u32> = ns.collect();
                #[allow(clippy::cast_precision_loss)]
                let w = 1.0 / ns.len() as f64;
                for n in ns {
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
        if stop(false) {
            return None;
        }
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
    if stop(false) {
        return None;
    }
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
    if stop(false) {
        return None;
    }
    synthesize(&glyphs, &mut gorg, ix);
    // (each glyph's origin its node's codes': its first and those after
    // it that are not a node's first)
    let mut orgs = vec![Org::NONE; g.codes.len()];
    for (gl, &o) in glyphs.iter().zip(&gorg) {
        let o = if o.node == NO_NODE {
            Org::NONE
        } else {
            Org {
                node: local(o.node),
                ..o
            }
        };
        orgs[gl.code] = o;
        for (x, c) in orgs.iter_mut().zip(&g.codes).skip(gl.code + 1) {
            if c.kind == GLYPH && c.first {
                break;
            }
            if c.kind == GLYPH {
                *x = o;
            }
        }
    }
    Some(Matched {
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
    })
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
    pub spans: Arc<[SpanRow]>,
    /// The document's name and the text its nodes hold (its canonical
    /// text where `doc_raw` is the text given).
    pub doc_name: String,
    pub doc_text: &'a str,
    pub doc_raw: Option<&'a str>,
    /// The pages' glyphs, by place in `shown`, where they were taken
    /// with the rest ([`OriginsCapture`]); none: the kept ones.
    pub glyphs: Option<&'a [Option<Arc<PageGlyphs>>]>,
    /// Which state of the Build this is (`crate::Build::origins_key`):
    /// the same key, the same spans, texts and pages; 0: not known, made
    /// afresh.
    pub key: u64,
}

/// A text node as placed: its file (index into the files) and start.
#[derive(Clone, Copy)]
struct Span {
    /// The node whose text it is (a file a node read inline: that
    /// node's, the span's own id another).
    of: u64,
    file: u32,
    s: u32,
    len: u32,
    /// The document's own text (its offsets go over to the raw text).
    doc: bool,
    /// The hash of its text.
    hash: u64,
}

/// A text node's span ([`crate::Build::spans_named`]): its raw id, its
/// file's name and text (`None`: the document's own), its bytes there,
/// the index of the document node it is under, and the node whose text
/// it is.
pub(crate) type SpanRow = (u64, Option<(String, Arc<str>)>, usize, usize, usize, u64);

/// What an answer is made with: the index, the spans by raw id, each
/// file's text (as held, and how offsets in it become the file's bytes).
struct Ctx<'a> {
    inp: Input<'a>,
    m: Arc<Made>,
}

/// What a [`Ctx`] is made of, made once for the Build's state an
/// [`Input::key`] names and kept: each page asked of that state answered
/// from it, not from a look at every node again.
struct Made {
    index: Arc<Index>,
    spans: FxHashMap<u64, Span>,
    /// Per file (by index): its text as the nodes hold it.
    texts: Vec<Option<Arc<str>>>,
    bytes: Mutex<FxHashMap<u32, Arc<Bytes>>>,
    canon: OnceLock<(Vec<u32>, Vec<u32>)>,
}

impl std::ops::Deref for Ctx<'_> {
    type Target = Made;
    fn deref(&self) -> &Made {
        &self.m
    }
}

/// The Build's origins memos: the nodes' scans, the index, the pages'
/// matches, the files named (append-only), the last answers given.
#[derive(Default)]
pub(crate) struct State {
    scans: Mutex<FxHashMap<u64, Arc<NodeScan>>>,
    index: Mutex<Option<(u64, Arc<Index>)>>,
    /// The pages' matches, by page and the glyphs they were made from.
    matches: Mutex<FxHashMap<u64, MatchKept>>,
    /// The pages' answers, by page and what they were made from.
    answers: Mutex<FxHashMap<u64, Ans>>,
    files: Mutex<Vec<String>>,
    last: Mutex<Last>,
    /// The PDF a DVI or XDV build's file became, as its client made it
    /// ([`crate::Build::set_origins_pdf`]): its forms' codes.
    pdf: Mutex<Option<Arc<Vec<u8>>>>,
    /// The last [`Made`], by the [`Input::key`] it was made for.
    made: Mutex<Option<(u64, Arc<Made>)>>,
}

/// A page's match and the glyphs it was made from.
type MatchKept = (Arc<PageGlyphs>, Arc<Matched>);

/// A page's answer ([`answer`]) and what it was made from: its match
/// and its nodes' texts (`key`), where each node of the match stood in
/// its file (`at`: its placed start and end, `u32::MAX` for a node gone),
/// and per code the node it came from (`of`, `u32::MAX` none).
struct Ans {
    key: u64,
    /// The glyphs and the match it was made from, held: compared by
    /// identity (a pointer in the key alone named a match freed and its
    /// memory given to another: a page's stale answer kept, para4000).
    g: Arc<PageGlyphs>,
    m: Arc<Matched>,
    at: Vec<(u32, u32)>,
    of: Arc<[u32]>,
    out: Arc<PageOrigins>,
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

    #[allow(clippy::too_many_lines)]
    fn ctx<'a>(&self, inp: Input<'a>) -> Ctx<'a> {
        self.ctx_until(inp, &|_| false).expect("never stopped")
    }

    /// [`State::ctx`], given up (`None`) once `stop` says so (every few
    /// hundred nodes: a keystroke waits for none of it).
    #[allow(clippy::too_many_lines)]
    fn ctx_until<'a>(&self, inp: Input<'a>, stop: &dyn Fn(bool) -> bool) -> Option<Ctx<'a>> {
        // (made already for this state of the Build: a page's answer looks
        // at its own nodes only)
        if inp.key != 0
            && let Some(m) = self
                .made
                .locked()
                .as_ref()
                .filter(|x| x.0 == inp.key)
                .map(|x| x.1.clone())
        {
            return Some(Ctx { inp, m });
        }
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
        for (n, &i) in order.iter().enumerate() {
            if n % 256 == 255 && stop(false) {
                return None;
            }
            let (raw, f, s, e, _, of) = &inp.spans[i];
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
                    of: *of,
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
        // (another set of nodes: the index of the last one updated, the
        // nodes the edit changed taken out and put in again, not made
        // anew from every node's words: para4000's edit, 180 ms a time)
        let index = index.unwrap_or_else(|| {
            let post = self
                .index
                .locked()
                .take()
                .map(|(_, ix)| match Arc::try_unwrap(ix) {
                    Ok(ix) => ix.post,
                    Err(ix) => ix.post.clone(),
                })
                .unwrap_or_default();
            let ix = Arc::new(Index::update(nodes, post));
            *self.index.locked() = Some((key, ix.clone()));
            ix
        });
        // (the document's own text, as given: where the nodes hold its
        // canonical text, offsets go over by line and column)
        let m = Arc::new(Made {
            index,
            spans,
            texts,
            bytes: Mutex::new(FxHashMap::default()),
            canon: OnceLock::new(),
        });
        if inp.key != 0 {
            *self.made.locked() = Some((inp.key, m.clone()));
        }
        Some(Ctx { inp, m })
    }

    /// Page `k`'s match, kept while its glyphs are the same and every
    /// node it names is there with the same text.
    fn matched(
        &self,
        cx: &Ctx<'_>,
        id: u64,
        g: &Arc<PageGlyphs>,
        stop: &dyn Fn(bool) -> bool,
    ) -> Option<Arc<Matched>> {
        // (kept while the page's glyphs are the same, held, and its
        // nodes' texts are)
        let kept = self.matches.locked().get(&id).cloned();
        if let Some((g0, m)) = kept
            && Arc::ptr_eq(&g0, g)
            && m.nodes
                .iter()
                .all(|&(raw, h)| cx.spans.get(&raw).is_some_and(|sp| sp.hash == h))
        {
            return Some(m);
        }
        let m = Arc::new(match_page_until(g, &cx.index, stop)?);
        let mut all = self.matches.locked();
        if all.len() > 1 << 16 {
            all.clear();
        }
        all.insert(id, (g.clone(), m.clone()));
        Some(m)
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
#[allow(clippy::too_many_lines)]
fn answer(
    st: &State,
    cx: &Ctx<'_>,
    k: usize,
    pdf: &dyn Fn() -> Arc<Vec<u8>>,
) -> Option<Arc<PageOrigins>> {
    answer_until(st, cx, k, pdf, &|_| false).unwrap_or(None)
}

/// [`answer`], given up (`Err`) once `stop` says so while the page is
/// matched.
#[allow(clippy::too_many_lines)]
fn answer_until(
    st: &State,
    cx: &Ctx<'_>,
    k: usize,
    pdf: &dyn Fn() -> Arc<Vec<u8>>,
    stop: &dyn Fn(bool) -> bool,
) -> Result<Option<Arc<PageOrigins>>, ()> {
    let Some(&id) = cx.inp.shown.get(k) else {
        return Ok(None);
    };
    let g = match cx.inp.glyphs {
        Some(gs) => gs.get(k).cloned().flatten(),
        None => kept(cx.inp.inst, id),
    };
    let Some(g) = g else {
        return Ok(None);
    };
    let Some(m) = st.matched(cx, id, &g, stop) else {
        return Err(());
    };
    let nodes: Vec<Option<(Span, bool)>> = m
        .nodes
        .iter()
        .map(|(raw, _)| cx.spans.get(raw).map(|&sp| (sp, sp.doc)))
        .collect();
    // (the answer kept while its match is the same, its nodes' texts are
    // and they stand where they stood; where they only moved, each code's
    // bytes moved with its node: the answer shifted, not made anew (an
    // edit moves every node after it in its file, and every page after
    // it was answered again: para4000, 350 of 692 pages, 130 ms))
    let key = nodes.iter().fold(hash_words(id, &[]), |h, n| match n {
        Some((sp, doc)) => hash_words(h, &[u64::from(sp.file), sp.hash, u64::from(*doc)]),
        None => hash_words(h, &[u64::MAX]),
    });
    let at: Vec<(u32, u32)> = nodes
        .iter()
        .map(|n| match n {
            Some((sp, doc)) => (
                cx.place(sp.file, *doc, sp.s),
                cx.place(sp.file, *doc, sp.s + sp.len),
            ),
            None => (u32::MAX, u32::MAX),
        })
        .collect();
    let kept = st
        .answers
        .locked()
        .get(&id)
        .filter(|a| a.key == key && Arc::ptr_eq(&a.g, &g) && Arc::ptr_eq(&a.m, &m))
        .map(|a| (a.at.clone(), a.of.clone(), a.out.clone()));
    // (`PHITEX_ORIGINS_CHECK`: an answer kept or shifted compared with
    // one made from nothing kept, the page matched again)
    let check = |how: &str, o: &PageOrigins| {
        if crate::exec::env_flag!("PHITEX_ORIGINS_CHECK") {
            let m = match_page(&g, &cx.index);
            let nodes: Vec<Option<(Span, bool)>> = m
                .nodes
                .iter()
                .map(|(raw, _)| cx.spans.get(raw).map(|&sp| (sp, sp.doc)))
                .collect();
            let fresh = answer_fresh(cx, k, &g, &m, &nodes, pdf);
            eprintln!(
                "origins check: page {k} {how}: {}",
                if fresh.0 == *o { "same" } else { "DIFFERENT" }
            );
        }
    };
    if let Some((old, of, out)) = kept {
        if old == at {
            check("kept", &out);
            return Ok(Some(out));
        }
        // (each node moved whole: its start and end by one distance)
        let d: Option<Vec<i64>> = old
            .iter()
            .zip(&at)
            .map(|(&(a0, b0), &(a1, b1))| {
                if a0 == u32::MAX || a1 == u32::MAX {
                    return (a0 == a1).then_some(0);
                }
                let d = i64::from(a1) - i64::from(a0);
                (i64::from(b1) - i64::from(b0) == d).then_some(d)
            })
            .collect();
        if let Some(d) = d {
            let mut o = (*out).clone();
            for (i, &n) in of.iter().enumerate() {
                if let Some(&dn) = d.get(n as usize)
                    && dn != 0
                {
                    o.start[i] = u32::try_from(i64::from(o.start[i]) + dn).unwrap_or(u32::MAX);
                    o.end[i] = u32::try_from(i64::from(o.end[i]) + dn).unwrap_or(u32::MAX);
                }
            }
            let o = Arc::new(o);
            check("shifted", &o);
            st.answers.locked().insert(
                id,
                Ans {
                    key,
                    g: g.clone(),
                    m: m.clone(),
                    at,
                    of,
                    out: o.clone(),
                },
            );
            return Ok(Some(o));
        }
    }
    let (out, of) = answer_fresh(cx, k, &g, &m, &nodes, pdf);
    let out = Arc::new(out);
    let mut all = st.answers.locked();
    if all.len() > 1 << 16 {
        all.clear();
    }
    all.insert(
        id,
        Ans {
            key,
            g: g.clone(),
            m: m.clone(),
            at,
            of: of.into(),
            out: out.clone(),
        },
    );
    Ok(Some(out))
}

/// Page `k`'s answer made from its glyphs `g` and match `m`, its nodes
/// placed as `nodes` says, with per code the node it came from.
fn answer_fresh(
    cx: &Ctx<'_>,
    k: usize,
    g: &PageGlyphs,
    m: &Matched,
    nodes: &[Option<(Span, bool)>],
    pdf: &dyn Fn() -> Arc<Vec<u8>>,
) -> (PageOrigins, Vec<u32>) {
    let told = |c: &Code| g.xcodes.iter().find(|&&(o, _)| o == c.lig).map(|&(_, n)| n);
    // (an object whose codes the writer did not tell, XeTeX's pictures
    // drawn by xdvipdfmx: counted in the PDF made)
    let counts = if g
        .codes
        .iter()
        .any(|c| c.kind == XOBJECT && told(c).is_none())
    {
        xobject_counts(&pdf(), k)
    } else {
        Vec::new()
    };
    let mut out = PageOrigins::default();
    let mut of: Vec<u32> = Vec::new();
    let (mut x, mut y) = (0.0, 0.0);
    let mut forms = counts.iter();
    for (c, o) in g.codes.iter().zip(&m.orgs) {
        match c.kind {
            GLYPH => {
                (x, y) = (bp(c.h), bp(c.v));
                let (w, z) = (bp(c.w), bp(c.size));
                if let Some((sp, doc)) = nodes.get(o.node as usize).copied().flatten() {
                    let a = cx.place(sp.file, doc, sp.s + o.from);
                    let b = cx.place(sp.file, doc, sp.s + o.to);
                    out.push(sp.file, a, b, o.synth, x, y, w, z);
                    of.push(o.node);
                } else {
                    out.push(u32::MAX, 0, 0, false, x, y, w, z);
                    of.push(u32::MAX);
                }
            }
            XOBJECT => {
                let pdf_n = forms.next().copied();
                for _ in 0..told(c).or(pdf_n).unwrap_or(0) {
                    out.push(u32::MAX, 0, 0, false, x, y, 0.0, 0.0);
                    of.push(u32::MAX);
                }
            }
            _ => {
                out.push(u32::MAX, 0, 0, false, x, y, 0.0, 0.0);
                of.push(u32::MAX);
            }
        }
    }
    (out, of)
}

/// [`crate::Build::pages_of_source`] of the state `cx` is made of.
fn pages_of(st: &State, cx: &Ctx<'_>, file: &str, at: u32) -> Vec<usize> {
    let file = file.trim_start_matches("./");
    let names = st.files.locked().clone();
    let Some(fi) = names
        .iter()
        .position(|n| n == file || format!("{n}.tex") == file)
        .and_then(|i| u32::try_from(i).ok())
    else {
        return Vec::new();
    };
    let raws: phitex_defuse::FxHashSet<i64> = cx
        .spans
        .iter()
        .filter(|(_, sp)| {
            sp.file == fi
                && cx.place(sp.file, sp.doc, sp.s) <= at
                && at < cx.place(sp.file, sp.doc, sp.s + sp.len).max(1)
        })
        .map(|(_, sp)| sp.of.cast_signed())
        .collect();
    if raws.is_empty() {
        return Vec::new();
    }
    let memo = piece_memo().locked();
    cx.inp
        .shown
        .iter()
        .enumerate()
        .filter(|&(_, &id)| {
            memo.get(&(cx.inp.inst, id))
                .is_some_and(|ps| ps.iter().any(|p| raws.contains(p)))
        })
        .map(|(k, _)| k)
        .collect()
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

    /// [`Self::page_origins`], given up (`None`) once `stop` says so,
    /// between a few hundred nodes of the sources' look or while the page
    /// is matched: a watch's save waits for none of it (a page's origins
    /// asked by a browser after an edit took 135 ms of the loop).
    pub fn page_origins_until(
        &self,
        k: usize,
        stop: &dyn Fn(bool) -> bool,
    ) -> Option<Option<Arc<PageOrigins>>> {
        let st = self.origins_state();
        let cx = st.ctx_until(self.origins_input(), stop)?;
        answer_until(st, &cx, k, &|| self.origins_pdf(), stop).ok()
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
        self.origins_snapshot_until(&|_| false)
            .unwrap_or_else(|| OriginsSnapshot {
                pages: Vec::new(),
                files: Vec::new(),
                texts: Vec::new(),
            })
    }

    /// [`Self::origins_snapshot`], given up between two pages once `stop`
    /// says so (`None`; the pages answered so far kept, the next call
    /// goes on from them).
    pub fn origins_snapshot_until(&self, stop: &dyn Fn(bool) -> bool) -> Option<OriginsSnapshot> {
        snapshot_until(
            self.origins_state(),
            self.origins_input(),
            &|| self.origins_pdf(),
            stop,
        )
    }

    /// What [`Self::origins_snapshot`] of the Build as it is now is made
    /// from, taken apart from it ([`OriginsCapture`]): its snapshot made
    /// later, on any thread, is this one whatever the Build has become
    /// meanwhile (a watch's way out writes the `.synctex.gz` of the PDF
    /// file it wrote while the next edit's build runs).
    #[must_use]
    pub fn origins_capture(&self) -> OriginsCapture {
        let t0 = std::time::Instant::now();
        let inp = self.origins_input();
        crate::timeline::note("origins: the input (the nodes' spans)", t0.elapsed());
        let t0 = std::time::Instant::now();
        let glyphs = inp.shown.iter().map(|&id| kept(inp.inst, id)).collect();
        crate::timeline::note("origins: the pages' glyphs", t0.elapsed());
        OriginsCapture {
            st: self.origins_shared(),
            inst: inp.inst,
            shown: inp.shown,
            spans: inp.spans,
            doc_name: inp.doc_name,
            doc_text: Arc::from(inp.doc_text),
            doc_raw: inp.doc_raw.map(Arc::from),
            glyphs,
            pdf: self.origins_pdf_later(),
            key: inp.key,
        }
    }

    /// [`Self::origins_pdf`], made when asked from what it is now.
    fn origins_pdf_later(&self) -> Arc<dyn Fn() -> Arc<Vec<u8>> + Send + Sync> {
        if let Some(p) = self.origins_state().pdf.locked().clone() {
            return Arc::new(move || p.clone());
        }
        self.output_bytes_later()
    }

    /// The pages (0 the first) whose sealed lines came from the nodes
    /// whose text holds byte `at` of `file` (as origins name it, `.tex`
    /// or not; its bytes as origins count them): found from where the
    /// nodes' lines were placed, no page matched (a forward search's
    /// page). Empty where none is known: the caller looks on the pages.
    #[must_use]
    pub fn pages_of_source(&self, file: &str, at: u32) -> Vec<usize> {
        let st = self.origins_state();
        pages_of(st, &st.ctx(self.origins_input()), file, at)
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
        let (_, changed, deltas) = self.origins_update();
        (changed, deltas)
    }

    /// Every page's origins ([`Self::all_page_origins`]) with the pages
    /// whose origins changed since the last call and the edits since then
    /// ([`Self::origins_changed`]), made with one look at the sources: a
    /// viewer told what changed and given the origins after a build (the
    /// two asked apart made the pages' origins twice and read the sources
    /// three times: Grail's 21 ms a keystroke in WebAssembly).
    #[allow(clippy::type_complexity)]
    pub fn origins_update(
        &self,
    ) -> (
        Vec<Option<Arc<PageOrigins>>>,
        Vec<usize>,
        Vec<(u32, u32, i64)>,
    ) {
        self.origins_update_until(&|_| false).unwrap_or_default()
    }

    /// [`Self::origins_update`], given up between two pages once `stop`
    /// says so (an event waits: the watch's next keystroke is not held
    /// behind the rematch of every page): `None`, nothing told; the
    /// pages answered so far are kept, the next call goes on from them.
    #[allow(clippy::type_complexity)]
    pub fn origins_update_until(
        &self,
        stop: &dyn Fn(bool) -> bool,
    ) -> Option<(
        Vec<Option<Arc<PageOrigins>>>,
        Vec<usize>,
        Vec<(u32, u32, i64)>,
    )> {
        let st = self.origins_state();
        let cx = st.ctx_until(self.origins_input(), stop)?;
        let pdf: OnceLock<Arc<Vec<u8>>> = OnceLock::new();
        let get = || pdf.get_or_init(|| self.origins_pdf()).clone();
        let mut pages: Vec<Option<Arc<PageOrigins>>> = Vec::with_capacity(cx.inp.shown.len());
        for k in 0..cx.inp.shown.len() {
            // (a page at least each call: a call always gets on)
            if stop(true) {
                return None;
            }
            pages.push(answer_until(st, &cx, k, &get, stop).ok()?);
        }
        let cx_texts: Vec<(u32, Arc<[u8]>)> = {
            let inp = &cx.inp;
            let mut ts: Vec<(u32, Arc<[u8]>)> = vec![(
                st.file_index(&inp.doc_name),
                file_bytes(inp.doc_raw.unwrap_or(inp.doc_text)),
            )];
            let mut seen: phitex_defuse::FxHashSet<String> = phitex_defuse::FxHashSet::default();
            for (_, f, ..) in inp.spans.iter() {
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
        last.pages.clone_from(&pages);
        last.texts = cx_texts.into_iter().collect();
        Some((pages, changed, deltas))
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

/// What [`crate::Build::origins_snapshot`] is made from, taken apart from
/// the Build ([`crate::Build::origins_capture`]): the origins' memos
/// (shared, each kept by what it was made from), the pages, the nodes'
/// texts and places, the pages' glyphs, and the PDF their codes are
/// counted in as it was.
pub struct OriginsCapture {
    st: Arc<State>,
    inst: u64,
    shown: Vec<u64>,
    #[allow(clippy::type_complexity)]
    spans: Arc<[SpanRow]>,
    doc_name: String,
    doc_text: Arc<str>,
    doc_raw: Option<Arc<str>>,
    glyphs: Vec<Option<Arc<PageGlyphs>>>,
    pdf: Arc<dyn Fn() -> Arc<Vec<u8>> + Send + Sync>,
    key: u64,
}

impl OriginsCapture {
    /// The input it was taken of.
    fn input(&self) -> Input<'_> {
        Input {
            inst: self.inst,
            shown: self.shown.clone(),
            spans: self.spans.clone(),
            doc_name: self.doc_name.clone(),
            doc_text: &self.doc_text,
            doc_raw: self.doc_raw.as_deref(),
            glyphs: Some(&self.glyphs),
            key: self.key,
        }
    }

    /// The Build's [`crate::Build::page_origins`] of page `k` when it was
    /// taken, made on any thread whatever the Build has become.
    #[must_use]
    pub fn page(&self, k: usize) -> Option<Arc<PageOrigins>> {
        let cx = self.st.ctx(self.input());
        answer(&self.st, &cx, k, &|| (self.pdf)())
    }

    /// The Build's [`crate::Build::pages_of_source`] when it was taken.
    #[must_use]
    pub fn pages_of_source(&self, file: &str, at: u32) -> Vec<usize> {
        let cx = self.st.ctx(self.input());
        pages_of(&self.st, &cx, file, at)
    }

    /// The files origins name ([`crate::Build::origin_files`]).
    #[must_use]
    pub fn files(&self) -> Vec<String> {
        self.st.files.locked().clone()
    }

    /// The Build's [`crate::Build::origins_snapshot`] when it was taken.
    #[must_use]
    pub fn snapshot(&self) -> OriginsSnapshot {
        let inp = self.input();
        snapshot_until(&self.st, inp, &|| (self.pdf)(), &|_| false).unwrap_or_else(|| {
            OriginsSnapshot {
                pages: Vec::new(),
                files: Vec::new(),
                texts: Vec::new(),
            }
        })
    }
}

/// [`crate::Build::origins_snapshot_until`] of input `inp` with memos
/// `st`, the PDF the codes are counted in from `pdf_of`.
fn snapshot_until(
    st: &State,
    inp: Input<'_>,
    pdf_of: &dyn Fn() -> Arc<Vec<u8>>,
    stop: &dyn Fn(bool) -> bool,
) -> Option<OriginsSnapshot> {
    let t0 = std::time::Instant::now();
    let t1 = t0.elapsed();
    let cx = st.ctx_until(inp, stop)?;
    let t2 = t0.elapsed();
    let pdf: OnceLock<Arc<Vec<u8>>> = OnceLock::new();
    let get = || pdf.get_or_init(pdf_of).clone();
    let mut pages = Vec::with_capacity(cx.inp.shown.len());
    for k in 0..cx.inp.shown.len() {
        if stop(true) {
            return None;
        }
        pages.push(answer_until(st, &cx, k, &get, stop).ok()?);
    }
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
    Some(OriginsSnapshot {
        pages,
        files,
        texts,
    })
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

    /// A text as a `Build` holds its file: one character a byte.
    fn held(t: &str) -> String {
        t.bytes().map(char::from).collect()
    }

    /// Native words told the tap as XeTeX's XDV writer tells them, each
    /// `(text, left edge)` at `v`, one glyph a character, 5 pt each.
    fn natives(r: &mut Recorder, ws: &[(&str, Scaled)], v: Scaled) {
        const PT: Scaled = 65536;
        for &(t, h) in ws {
            let text: Vec<u16> = t.encode_utf16().collect();
            let n = Scaled::try_from(t.chars().count()).unwrap_or(1);
            // (a right-to-left word's glyphs as the page shows them)
            let gs: Vec<(Scaled, Scaled, Scaled)> =
                (0..n).map(|k| (h + k * 5 * PT, v, 5 * PT)).collect();
            r.native(0, &text, &gs, 10 * PT);
        }
    }

    /// The source range (offsets in its node's text) of each glyph of
    /// `g` placed, not synthesized, in the page's order of its glyphs.
    fn placed(g: &PageGlyphs, ix: &Index) -> Vec<(u32, u32)> {
        let m = match_page(g, ix);
        m.orgs
            .iter()
            .zip(&g.codes)
            .filter(|(o, c)| c.first && o.node != NO_NODE && !o.synth)
            .map(|(o, _)| (o.from, o.to))
            .collect()
    }

    /// Right-to-left text, shown by TeX--XeT's writer in the reverse of
    /// its reading order (the tap's level 1): each word of the page is
    /// matched to its own occurrence in the source, the repeated word
    /// too. Read in the page's order, the leftmost word (the last one
    /// read) took the source's first.
    #[test]
    fn right_to_left_words_match_their_own_occurrence() {
        const PT: Scaled = 65536;
        let src = held("Before \\textlang{arabic}{كلمة أخرى كلمة} after.");
        let ix = Index::update(
            vec![Node {
                raw: 1,
                scan: Arc::new(scan(&src)),
            }],
            Arc::default(),
        );
        let (mut r, out) = Recorder::new();
        natives(&mut r, &[("Before", 0)], 0);
        r.level(1);
        // (the page's order: the last word read is leftmost)
        natives(
            &mut r,
            &[("كلمة", 40 * PT), ("أخرى", 65 * PT), ("كلمة", 90 * PT)],
            0,
        );
        r.level(0);
        natives(&mut r, &[("after.", 120 * PT)], 0);
        drop(r);
        let g = out.lock().unwrap().take().unwrap();
        let word = |w: &str, k: usize| {
            let w = held(w);
            let at = src.match_indices(&w).nth(k).unwrap().0;
            (
                u32::try_from(at).unwrap(),
                u32::try_from(at + w.len()).unwrap(),
            )
        };
        // (in the page's order: the second كلمة, أخرى, the first كلمة)
        assert_eq!(
            placed(&g, &ix),
            [
                word("Before", 0),
                word("كلمة", 1),
                word("أخرى", 0),
                word("كلمة", 0),
                word("after.", 0),
            ]
        );
    }

    /// Two paragraphs of the same right-to-left sentence, each a line of
    /// its own (a context each): each line matched to its own paragraph.
    #[test]
    fn repeated_right_to_left_paragraphs_match_their_own() {
        const PT: Scaled = 65536;
        let para = "يكتب الحاسوب الصفحة";
        let texts = [held(&format!("{para}\n")), held(&format!("{para}\n"))];
        let ix = Index::update(
            texts
                .iter()
                .enumerate()
                .map(|(k, t)| Node {
                    raw: k as u64 + 1,
                    scan: Arc::new(scan(t)),
                })
                .collect(),
            Arc::default(),
        );
        let (mut r, out) = Recorder::new();
        for line in 0..2 {
            r.level(1);
            let v = line * 12 * PT;
            natives(
                &mut r,
                &[("الصفحة", 0), ("الحاسوب", 40 * PT), ("يكتب", 80 * PT)],
                v,
            );
            r.level(0);
        }
        drop(r);
        let g = out.lock().unwrap().take().unwrap();
        let m = match_page(&g, &ix);
        let nodes: Vec<u64> = m
            .orgs
            .iter()
            .zip(&g.codes)
            .filter(|(o, c)| c.first && o.node != NO_NODE)
            .map(|(o, _)| m.nodes[o.node as usize].0)
            .collect();
        assert_eq!(nodes, [1, 1, 1, 2, 2, 2], "{:?}", m.orgs);
        // (each line read from its right: the page's first word, leftmost,
        // is the paragraph's last)
        let first = m.orgs[0];
        let t = &texts[0];
        assert_eq!(&t[first.from as usize..first.to as usize], held("الصفحة"));
    }

    #[test]
    fn latin1_offsets_are_bytes() {
        let t: String = [b'a', 0xE9, b'b'].iter().map(|&b| char::from(b)).collect();
        let b = Bytes::of(&t);
        assert_eq!(b.at(3), 2);
        assert_eq!(b.at(1), 1);
    }
}
