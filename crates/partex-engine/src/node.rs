//! Typed nodes: what horizontal and vertical lists are made of (tex.web
//! part 10, redesigned).
//!
//! A list is a `Vec<Node>`. Boxes are shared and immutable once built
//! (`Arc<BoxNode>`; changing one, as `\wd` does, copies it on write), so
//! `\copy`, box registers and caches share them for free and whole pages
//! can move to another thread. Glue specifications are plain values.
//!
//! The numbers TeX shows (glue and kern subtypes, `\lastnodetype`) are kept
//! as data where TeX's output depends on them.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use core::fmt::Write;

use crate::Scaled;

/// An internal font number, as TeX numbers fonts (`null_font` is 0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FontId(pub u16);

impl FontId {
    pub const NULL: FontId = FontId(0);
}

/// §150: the order of infinity of a glue stretch or shrink.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Order {
    #[default]
    Normal,
    Fil,
    Fill,
    Filll,
}

impl Order {
    pub const ALL: [Order; 4] = [Order::Normal, Order::Fil, Order::Fill, Order::Filll];

    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }
}

/// §150: a glue specification.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GlueSpec {
    pub width: Scaled,
    pub stretch: Scaled,
    pub shrink: Scaled,
    pub stretch_order: Order,
    pub shrink_order: Order,
    /// This is TeX's shared `zero_glue` (§162), not just a spec whose
    /// values are zero. tex.web compares against it by pointer: glue of
    /// a zero parameter or register is it, `\hskip 0pt` is not, and
    /// `short_display` shows a space for the latter only (§175).
    pub shared_zero: bool,
}

impl GlueSpec {
    /// §162: `zero_glue`.
    pub const ZERO_GLUE: GlueSpec = GlueSpec {
        width: 0,
        stretch: 0,
        shrink: 0,
        stretch_order: Order::Normal,
        shrink_order: Order::Normal,
        shared_zero: true,
    };

    /// A new spec with these values (tex.web's `new_spec`): never the
    /// shared zero glue.
    #[must_use]
    pub const fn copy(self) -> GlueSpec {
        GlueSpec {
            shared_zero: false,
            ..self
        }
    }

    /// Are all components zero (§1229's `trap_zero_glue` test)?
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.width == 0 && self.stretch == 0 && self.shrink == 0
    }
}

/// §135: how a box's glue is set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GlueSign {
    #[default]
    Normal,
    Stretching,
    Shrinking,
}

/// §138: a rule dimension that is "running" (taken from the enclosing box).
pub const RUNNING: Scaled = -0o10000000000;

/// §134: a run of characters in one font (tex.web has one node per
/// character). Lists keep runs canonical: a maximal sequence of same-font
/// characters is split into runs of [`Glyphs::CAP`] from its start (see
/// [`push_char`]), so structural equality and hashing are character-level.
#[derive(Clone, Copy)]
pub struct Glyphs {
    pub font: FontId,
    len: u8,
    chars: [u8; Glyphs::CAP],
}

/// By the characters in the run: bytes past `len` are left over from
/// removed characters, not part of the value.
impl PartialEq for Glyphs {
    fn eq(&self, other: &Self) -> bool {
        self.font == other.font && self.chars() == other.chars()
    }
}

impl Eq for Glyphs {}

impl core::hash::Hash for Glyphs {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.font.hash(h);
        self.chars().hash(h);
    }
}

impl Glyphs {
    /// Characters per run (what fits in a 24-byte node).
    pub const CAP: usize = 19;

    #[must_use]
    pub fn one(font: FontId, ch: u8) -> Glyphs {
        let mut chars = [0; Glyphs::CAP];
        chars[0] = ch;
        Glyphs {
            font,
            len: 1,
            chars,
        }
    }

    #[must_use]
    pub fn chars(&self) -> &[u8] {
        &self.chars[..usize::from(self.len)]
    }

    #[must_use]
    pub fn is_full(&self) -> bool {
        usize::from(self.len) == Glyphs::CAP
    }

    /// Append `ch` (the run must not be full).
    pub fn push(&mut self, ch: u8) {
        self.chars[usize::from(self.len)] = ch;
        self.len += 1;
    }

    /// Remove and return the last character; the run must not become
    /// empty.
    pub fn pop(&mut self) -> Option<u8> {
        if self.len <= 1 {
            return None;
        }
        self.len -= 1;
        Some(self.chars[usize::from(self.len)])
    }
}

impl core::fmt::Debug for Glyphs {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Glyphs(f{} {:?})", self.font.0, self.chars())
    }
}

/// Append character `ch` of `font` to `list`, keeping runs canonical.
pub fn push_char(list: &mut Vec<Node>, font: FontId, ch: u8) {
    if let Some(Node::Glyphs(g)) = list.last_mut()
        && g.font == font
        && !g.is_full()
    {
        g.push(ch);
        return;
    }
    list.push(Node::Glyphs(Glyphs::one(font, ch)));
}

/// Are the glyph runs of `list` (not nested lists) canonical?
#[must_use]
pub fn is_canonical(list: &[Node]) -> bool {
    list.windows(2).all(|w| match w {
        [Node::Glyphs(a), Node::Glyphs(b)] => a.font != b.font || a.is_full(),
        _ => true,
    })
}

/// Whether boxes carry versions ([`BoxNode::ver`]): set by a build that
/// records (DESIGN 7.17.12, the structures' convention), off for a plain
/// run, which then pays nothing.
pub static VERSIONS: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// §135: an `hlist_node` or `vlist_node`.
#[derive(Clone, Debug, Default)]
pub struct BoxNode {
    pub vertical: bool,
    pub width: Scaled,
    pub height: Scaled,
    pub depth: Scaled,
    /// `shift_amount`: down (hlist in a vlist: right) by this much.
    pub shift: Scaled,
    /// `glue_set`, a ratio TeX keeps as a double.
    pub glue_set: f64,
    pub glue_sign: GlueSign,
    pub glue_order: Order,
    /// The box's `subtype` (e-TeX's LR mode; 0 in TeX).
    pub subtype: u8,
    pub list: Vec<Node>,
    /// A sealed line (machine mode, `DESIGN.md` §7.0): its glue setting
    /// and list are kept under this key outside the box, which holds
    /// neither (only what vertical lists observe, its dimensions).
    pub seal: Option<u128>,
    /// The box's version (with [`VERSIONS`]; 0 without): made when the
    /// box becomes a shared value ([`BoxNode::share`]) from its parts, its
    /// dimensions, glue setting and list, whose boxes give their own
    /// versions, so a box costs what its own list holds (DESIGN 7.17.12).
    /// A box changed after that (`\\wd`, a shift) is versioned again
    /// ([`BoxNode::reversion`]).
    pub ver: u128,
}

/// Boxes are equal by content; the version is a name for the content.
impl PartialEq for BoxNode {
    fn eq(&self, o: &Self) -> bool {
        self.vertical == o.vertical
            && self.width == o.width
            && self.height == o.height
            && self.depth == o.depth
            && self.shift == o.shift
            && self.glue_set.to_bits() == o.glue_set.to_bits()
            && self.glue_sign == o.glue_sign
            && self.glue_order == o.glue_order
            && self.subtype == o.subtype
            && self.list == o.list
            && self.seal == o.seal
    }
}

impl BoxNode {
    /// The version of the box's parts: its fields, its list's nodes with
    /// each box by its own version (DESIGN 7.17.12).
    #[must_use]
    pub fn parts_version(&self) -> u128 {
        let mut h = crate::stablehash::StableHasher::new();
        core::hash::Hash::hash(&(2u8,), &mut h);
        self.hash_parts(&mut h);
        h.finish128() | 1
    }

    /// The box as a shared value, carrying its version (with
    /// [`VERSIONS`]).
    #[must_use]
    pub fn share(mut self) -> Arc<BoxNode> {
        self.reversion();
        Arc::new(self)
    }

    /// Make the box's version again from its parts (with [`VERSIONS`]),
    /// after a change to a box already made.
    pub fn reversion(&mut self) {
        if VERSIONS.load(core::sync::atomic::Ordering::Relaxed) {
            self.ver = self.parts_version();
        }
    }

    fn hash_parts<H: core::hash::Hasher>(&self, h: &mut H) {
        use core::hash::Hash;
        self.vertical.hash(h);
        self.width.hash(h);
        self.height.hash(h);
        self.depth.hash(h);
        self.shift.hash(h);
        // TeX computes the ratio deterministically; equal boxes have
        // equal bits.
        self.glue_set.to_bits().hash(h);
        self.glue_sign.hash(h);
        self.glue_order.hash(h);
        self.subtype.hash(h);
        self.list.hash(h);
        if let Some(k) = self.seal {
            k.hash(h);
        }
    }
}

impl core::hash::Hash for BoxNode {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        if self.ver != 0 {
            // (a versioned box by its version: its parts, once)
            self.ver.hash(h);
            return;
        }
        self.hash_parts(h);
    }
}

/// §149: leader kinds (`a_leaders`, `c_leaders`, `x_leaders`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Leaders {
    Aligned,
    Centered,
    Expanded,
}

/// A token list as a value (DESIGN 7.17.12, the structures' convention):
/// immutable once made, shared (`Arc`), carrying the version it was made
/// with. Macro bodies, token registers and parameters, marks, `\write`
/// texts, macro arguments and the lists the input reads are all this
/// value. The token encoding belongs to whoever owns the control sequence
/// table.
#[derive(Clone, Default)]
pub struct TokenList {
    toks: Vec<i32>,
    protected: bool,
    ver: u128,
}

/// A shared token list.
pub type Tokens = Arc<TokenList>;

/// 2^61 − 1: the modulus of a list's polynomial.
const TOK_P: u64 = (1 << 61) - 1;
/// The polynomial's base.
const TOK_B: u64 = 0x0e3f_5b79_a1c3_e5f7 % TOK_P;

/// The polynomial of a list with one more token `t` after those of `h`.
#[inline]
#[must_use]
pub fn tok_poly_step(h: u64, t: i32) -> u64 {
    // (no 128-bit product on wasm32: `mulmod61` makes it from halves there)
    let m = partex_ssa::hash::mulmod61(h, TOK_B);
    let mut s = m + u64::from(t.cast_unsigned()) + 1;
    if s >= TOK_P {
        s -= TOK_P;
    }
    s
}

/// The version of a token list: the polynomial of its tokens, its length
/// and its `\protected` flag (equal iff the tokens and the flag are, up to
/// the polynomial's collisions, 2^-61 a pair).
#[must_use]
pub fn tok_version(toks: &[i32], protected: bool) -> u128 {
    let poly = toks.iter().fold(0, |h, &t| tok_poly_step(h, t));
    let len = u128::try_from(toks.len()).unwrap_or(u128::MAX >> 64);
    (u128::from(poly) << 64) | (len << 1) | u128::from(protected)
}

impl TokenList {
    /// The list of `toks`, versioned now (with [`VERSIONS`]).
    #[must_use]
    pub fn new(toks: Vec<i32>, protected: bool) -> Self {
        let ver = if VERSIONS.load(core::sync::atomic::Ordering::Relaxed) {
            tok_version(&toks, protected)
        } else {
            0
        };
        TokenList {
            toks,
            protected,
            ver,
        }
    }

    /// A shared list of `toks`.
    #[must_use]
    pub fn shared(toks: &[i32]) -> Tokens {
        Arc::new(Self::new(toks.to_vec(), false))
    }

    /// The tokens.
    #[must_use]
    pub fn tokens(&self) -> &[i32] {
        &self.toks
    }

    /// e-TeX's `\protected` flag of a macro body.
    #[must_use]
    pub fn protected(&self) -> bool {
        self.protected
    }

    /// The version the list was made with (0 without [`VERSIONS`]).
    #[must_use]
    pub fn version(&self) -> u128 {
        self.ver
    }

    /// The version made again from the tokens (check mode's test).
    #[must_use]
    pub fn version_by_tokens(&self) -> u128 {
        tok_version(&self.toks, self.protected)
    }

    /// The tokens, to reuse the allocation of a list no one else holds
    /// (a pooled macro argument): the list is emptied and must be made
    /// again with [`TokenList::remake`].
    pub fn buffer(&mut self) -> &mut Vec<i32> {
        &mut self.toks
    }

    /// The list made again from its (changed) tokens, as a pooled list
    /// is (a macro's argument, tokens backed up or inserted): an input
    /// level only, never a value whose version is read, so none is made
    /// (hashing every argument's tokens was 2.4% of a keystroke natively,
    /// and more in wasm, whose 128-bit product is made from halves).
    pub fn remake(&mut self, protected: bool) {
        self.protected = protected;
        self.ver = 0;
    }
}

impl core::ops::Deref for TokenList {
    type Target = [i32];
    fn deref(&self) -> &[i32] {
        &self.toks
    }
}

impl PartialEq for TokenList {
    fn eq(&self, o: &Self) -> bool {
        self.protected == o.protected && self.toks == o.toks
    }
}

impl Eq for TokenList {}

impl core::hash::Hash for TokenList {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        if self.ver != 0 {
            // (a versioned list by its version: its tokens, once)
            self.ver.hash(h);
            return;
        }
        (self.protected, &self.toks).hash(h);
    }
}

impl crate::persist::Persist for TokenList {
    fn save(&self, s: &mut crate::persist::Saver) {
        self.toks.save(s);
        self.protected.save(s);
    }
    fn load(l: &mut crate::persist::Loader) -> Option<Self> {
        let toks: Vec<i32> = crate::persist::Persist::load(l)?;
        let protected: bool = crate::persist::Persist::load(l)?;
        Some(TokenList::new(toks, protected))
    }
}

impl fmt::Debug for TokenList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TokenList({:?}, protected: {})",
            self.toks, self.protected
        )
    }
}

#[derive(Clone, Debug, PartialEq, Hash)]
pub enum Node {
    /// §134: characters.
    Glyphs(Glyphs),
    /// §135: a box.
    Box(Arc<BoxNode>),
    /// §138: a rule; dimensions may be [`RUNNING`].
    Rule {
        width: Scaled,
        height: Scaled,
        depth: Scaled,
    },
    /// §149: glue. `subtype` is TeX's: 0, a glue parameter number plus 1,
    /// `cond_math_glue` (98) or `mu_glue` (99).
    Glue { spec: GlueSpec, subtype: u8 },
    /// §149: leaders: glue filled with a box or rule (`leader`).
    Leaders(alloc::boxed::Box<LeaderNode>),
    /// §155: a kern; `subtype` is `normal`, `explicit`, `acc_kern` or
    /// `mu_glue`.
    Kern { width: Scaled, subtype: u8 },
    /// pdfTeX's margin kern: a character's protrusion into the margin,
    /// put at the start (`left`) or end of a line.
    MarginKern {
        width: Scaled,
        left: bool,
        font: FontId,
        ch: u8,
    },
    /// §157
    Penalty(i32),
    /// §147: math on/off; `subtype` is `before`/`after` (plus e-TeX's LR
    /// codes).
    Math { width: Scaled, subtype: u8 },
    /// §143
    Ligature(alloc::boxed::Box<Ligature>),
    /// §145: a discretionary. The nodes it replaces when broken are inside
    /// it, not after it as in tex.web.
    Disc(alloc::boxed::Box<Disc>),
    /// §140
    Ins(alloc::boxed::Box<Ins>),
    /// §141
    Mark(alloc::boxed::Box<Mark>),
    /// §142: `\vadjust` material (pdfTeX's `\vadjust pre` when `pre`).
    Adjust(alloc::boxed::Box<Adjust>),
    /// §1341
    Whatsit(alloc::boxed::Box<Whatsit>),
    /// §159: an unset box of an alignment.
    Unset(alloc::boxed::Box<Unset>),
}

#[derive(Clone, Debug, PartialEq, Hash)]
pub struct LeaderNode {
    pub spec: GlueSpec,
    pub kind: Leaders,
    /// A box or rule node.
    pub leader: Node,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Ligature {
    pub font: FontId,
    pub ch: u8,
    /// §143: 1 if the ligature was formed with a left boundary, +2 with a
    /// right one.
    pub subtype: u8,
    /// The characters the ligature stands for.
    pub original: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Hash)]
pub struct Disc {
    pub pre: Vec<Node>,
    pub post: Vec<Node>,
    /// The nodes this discretionary replaces if a line breaks at it.
    pub replace: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Hash)]
pub struct Ins {
    pub number: u8,
    /// Natural height plus depth of the material.
    pub height: Scaled,
    pub split_top: GlueSpec,
    pub split_max_depth: Scaled,
    pub float_cost: i32,
    pub list: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Mark {
    /// e-TeX's mark class (0 for `\mark`).
    pub class: i32,
    pub tokens: Tokens,
}

#[derive(Clone, Debug, PartialEq, Hash)]
pub struct Adjust {
    pub pre: bool,
    pub list: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Whatsit {
    Open {
        stream: i32,
        name: Arc<[u8]>,
        area: Arc<[u8]>,
        ext: Arc<[u8]>,
    },
    Write {
        stream: i32,
        tokens: Tokens,
    },
    Close {
        stream: i32,
    },
    Special {
        tokens: Tokens,
    },
    Language {
        language: i32,
        left_hyphen_min: u8,
        right_hyphen_min: u8,
    },
    /// pdfTeX's `\special shipout` (expanded at shipping time).
    LateSpecial {
        tokens: Tokens,
    },
    /// pdfTeX's PDF whatsits.
    Pdf(Box<PdfWhatsit>),
}

impl Whatsit {
    /// The dimensions of a `\\pdfrefxform` or `\\pdfrefximage`, which
    /// pdfTeX's packing, line breaking and display widths count.
    #[must_use]
    pub fn ref_dims(&self) -> Option<Dims> {
        match self {
            Self::Pdf(p) => match **p {
                PdfWhatsit::RefXForm { dims, .. } | PdfWhatsit::RefXImage { dims, .. } => {
                    Some(dims)
                }
                _ => None,
            },
            _ => None,
        }
    }
}

/// A `<rule spec>` of a pdfTeX whatsit; `RUNNING` where not given.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Dims {
    pub width: Scaled,
    pub height: Scaled,
    pub depth: Scaled,
}

crate::persist_struct!(Dims {
    width,
    height,
    depth
});

/// A destination or thread identifier: `num n` or `name {...}`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PdfId {
    Num(i32),
    Name(Tokens),
}

crate::persist_enum!(PdfId { Num(a0), Name(a0) });

/// pdfTeX's action specification (`pdf_action_*`, pdfTeX §1554),
/// shared by the nodes it is copied into (pdfTeX counts references).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Action {
    /// `pdf_action_page`, `_goto`, `_thread` or `_user` (0–3).
    pub kind: u8,
    /// The `user` action's tokens, or a `page` action's.
    pub tokens: Option<Tokens>,
    /// `goto`/`thread`: the identifier (`page`: `Num` of the page).
    pub id: PdfId,
    pub file: Option<Tokens>,
    /// `struct`: a structure destination.
    pub struct_id: Option<PdfId>,
    /// 0, or 1 for `newwindow`, 2 for `nonewwindow`.
    pub new_window: u8,
}

crate::persist_struct!(Action {
    kind,
    tokens,
    id,
    file,
    struct_id,
    new_window
});

/// The whatsits pdfTeX adds (pdfTeX §695's node layouts).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PdfWhatsit {
    /// `\pdfliteral`; `mode` is `set_origin`, `direct_page` or
    /// `direct_always` (0–2).
    Literal {
        late: bool,
        mode: u8,
        data: Tokens,
    },
    /// `\pdfcolorstack`; `cmd` is set, push, pop or current (0–3).
    ColorStack {
        stack: i32,
        cmd: u8,
        data: Option<Tokens>,
    },
    SetMatrix {
        data: Tokens,
    },
    Save,
    Restore,
    RefObj {
        objnum: i32,
    },
    RefXForm {
        objnum: i32,
        dims: Dims,
    },
    RefXImage {
        objnum: i32,
        dims: Dims,
    },
    Annot {
        dims: Dims,
        data: Tokens,
        objnum: i32,
    },
    StartLink {
        dims: Dims,
        attr: Option<Tokens>,
        action: Arc<Action>,
        objnum: i32,
    },
    EndLink,
    Dest {
        dims: Dims,
        /// `struct n`.
        struct_num: Option<i32>,
        id: PdfId,
        /// `pdf_dest_xyz` ... `pdf_dest_fitr` (0–7).
        kind: u8,
        zoom: Option<i32>,
    },
    /// `\pdfthread` or (`start`) `\pdfstartthread`.
    Thread {
        start: bool,
        dims: Dims,
        attr: Option<Tokens>,
        id: PdfId,
    },
    EndThread,
    SavePos,
    SnapRefPoint,
    SnapY {
        glue: GlueSpec,
        final_skip: Scaled,
    },
    SnapYComp {
        ratio: i32,
    },
    InterwordSpaceOn,
    InterwordSpaceOff,
    FakeSpace,
    RunningLinkOff,
    RunningLinkOn,
}

#[derive(Clone, Debug, Default, PartialEq, Hash)]
pub struct Unset {
    pub width: Scaled,
    pub height: Scaled,
    pub depth: Scaled,
    /// The number of columns spanned, minus one.
    pub span_count: u16,
    pub stretch: Scaled,
    pub shrink: Scaled,
    pub stretch_order: Order,
    pub shrink_order: Order,
    pub list: Vec<Node>,
}

/// A stable text form of a list, for tests and debugging (not TeX's
/// `\showbox` format).
#[must_use]
pub fn dump(list: &[Node]) -> String {
    let mut s = String::new();
    dump_into(&mut s, list, 0);
    s
}

fn spec_text(g: &GlueSpec) -> String {
    let mut s = String::new();
    let _ = write!(s, "{}", g.width);
    if g.stretch != 0 {
        let _ = write!(s, " plus {}{:?}", g.stretch, g.stretch_order);
    }
    if g.shrink != 0 {
        let _ = write!(s, " minus {}{:?}", g.shrink, g.shrink_order);
    }
    s
}

fn dump_into(s: &mut String, list: &[Node], depth: usize) {
    let ind = depth * 2;
    for n in list {
        match n {
            Node::Glyphs(g) => {
                let text: String = g
                    .chars()
                    .iter()
                    .map(|&c| {
                        if c.is_ascii_graphic() {
                            char::from(c)
                        } else {
                            '?'
                        }
                    })
                    .collect();
                let _ = writeln!(
                    s,
                    "{:ind$}glyphs f{} {:?} {text:?}",
                    "",
                    g.font.0,
                    g.chars()
                );
            }
            Node::Box(b) => {
                let _ = write!(
                    s,
                    "{:ind$}{} wd={} ht={} dp={} shift={}",
                    "",
                    if b.vertical { "vbox" } else { "hbox" },
                    b.width,
                    b.height,
                    b.depth,
                    b.shift
                );
                if b.glue_sign != GlueSign::Normal {
                    let _ = write!(
                        s,
                        " glue={:?} {} {:?}",
                        b.glue_sign, b.glue_set, b.glue_order
                    );
                }
                if b.subtype != 0 {
                    let _ = write!(s, " subtype={}", b.subtype);
                }
                s.push('\n');
                dump_into(s, &b.list, depth + 1);
            }
            Node::Rule {
                width,
                height,
                depth: dp,
            } => {
                let _ = writeln!(s, "{:ind$}rule wd={width} ht={height} dp={dp}", "");
            }
            Node::Glue { spec, subtype } => {
                let _ = writeln!(s, "{:ind$}glue({subtype}) {}", "", spec_text(spec));
            }
            Node::Leaders(l) => {
                let _ = writeln!(s, "{:ind$}leaders {:?} {}", "", l.kind, spec_text(&l.spec));
                dump_into(s, core::slice::from_ref(&l.leader), depth + 1);
            }
            Node::Kern { width, subtype } => {
                let _ = writeln!(s, "{:ind$}kern({subtype}) {width}", "");
            }
            Node::MarginKern {
                width,
                left,
                font,
                ch,
            } => {
                let side = if *left { "left" } else { "right" };
                let _ = writeln!(s, "{:ind$}marginkern({side}) {width} f{} {ch}", "", font.0);
            }
            Node::Penalty(p) => {
                let _ = writeln!(s, "{:ind$}penalty {p}", "");
            }
            Node::Math { width, subtype } => {
                let _ = writeln!(s, "{:ind$}math({subtype}) {width}", "");
            }
            Node::Ligature(l) => {
                let _ = writeln!(
                    s,
                    "{:ind$}lig f{} {} ({}) {:?}",
                    "", l.font.0, l.ch, l.subtype, l.original
                );
            }
            Node::Disc(d) => {
                let _ = writeln!(s, "{:ind$}disc", "");
                for (name, l) in [("pre", &d.pre), ("post", &d.post), ("replace", &d.replace)] {
                    if !l.is_empty() {
                        let _ = writeln!(s, "{:w$}{name}:", "", w = ind + 2);
                        dump_into(s, l, depth + 2);
                    }
                }
            }
            Node::Ins(i) => {
                let _ = writeln!(
                    s,
                    "{:ind$}ins{} ht={} split={} {} cost={}",
                    "",
                    i.number,
                    i.height,
                    spec_text(&i.split_top),
                    i.split_max_depth,
                    i.float_cost
                );
                dump_into(s, &i.list, depth + 1);
            }
            Node::Mark(m) => {
                let _ = writeln!(s, "{:ind$}mark{} {:?}", "", m.class, m.tokens);
            }
            Node::Adjust(a) => {
                let _ = writeln!(s, "{:ind$}vadjust{}", "", if a.pre { " pre" } else { "" });
                dump_into(s, &a.list, depth + 1);
            }
            Node::Whatsit(w) => {
                let _ = writeln!(s, "{:ind$}whatsit {w:?}", "");
            }
            Node::Unset(u) => {
                let _ = writeln!(
                    s,
                    "{:ind$}unset wd={} ht={} dp={} span={} stretch={}{:?} shrink={}{:?}",
                    "",
                    u.width,
                    u.height,
                    u.depth,
                    u.span_count,
                    u.stretch,
                    u.stretch_order,
                    u.shrink,
                    u.shrink_order
                );
                dump_into(s, &u.list, depth + 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nodes_stay_small() {
        assert!(core::mem::size_of::<Node>() <= 24);
    }

    #[test]
    fn glyph_runs_are_canonical() {
        let (a, b) = (FontId(1), FontId(2));
        let mut list = Vec::new();
        for i in 0..45u8 {
            push_char(&mut list, a, i);
        }
        push_char(&mut list, b, 7);
        push_char(&mut list, a, 8);
        let lens: Vec<usize> = list
            .iter()
            .map(|n| match n {
                Node::Glyphs(g) => g.chars().len(),
                _ => 0,
            })
            .collect();
        assert_eq!(lens, [19, 19, 7, 1, 1]);
        assert!(is_canonical(&list));
        list.swap(0, 2);
        assert!(!is_canonical(&list));
    }
}
