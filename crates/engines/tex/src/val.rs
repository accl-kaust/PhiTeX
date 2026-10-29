//! Values: tokens, meanings, glue and the codes TeX gives them.
//!
//! A token is `Int(cat << 21 | char)` for a character (an active character
//! has category 13) or `Str("\\name")` for a control sequence (§289).
//! A meaning is a `List` tagged by its first element (§222–§225).

use crate::exec::LockWait as _;
use std::rc::Rc;
use std::sync::OnceLock;

use phitex_defuse::{Field, FxHashMap, Value, ValueId, Values};
use phitex_typeset::glue::{Glue, Order};

// Command codes (§207–§210), the table whole.
#[allow(dead_code)]
pub(crate) mod cmd {
    pub const RELAX: u8 = 0;
    pub const LEFT_BRACE: u8 = 1;
    pub const RIGHT_BRACE: u8 = 2;
    pub const MATH_SHIFT: u8 = 3;
    pub const TAB_MARK: u8 = 4;
    pub const CAR_RET: u8 = 5;
    pub const MAC_PARAM: u8 = 6;
    pub const SUP_MARK: u8 = 7;
    pub const SUB_MARK: u8 = 8;
    pub const IGNORE: u8 = 9;
    pub const SPACER: u8 = 10;
    pub const LETTER: u8 = 11;
    pub const OTHER_CHAR: u8 = 12;
    pub const ACTIVE_CHAR: u8 = 13;
    pub const PAR_END: u8 = 13;
    pub const COMMENT: u8 = 14;
    pub const STOP: u8 = 14;
    pub const INVALID_CHAR: u8 = 15;
    pub const DELIM_NUM: u8 = 15;
    pub const CHAR_NUM: u8 = 16;
    pub const MATH_CHAR_NUM: u8 = 17;
    pub const MARK: u8 = 18;
    pub const XRAY: u8 = 19;
    pub const MAKE_BOX: u8 = 20;
    pub const HMOVE: u8 = 21;
    pub const VMOVE: u8 = 22;
    pub const UN_HBOX: u8 = 23;
    pub const UN_VBOX: u8 = 24;
    pub const REMOVE_ITEM: u8 = 25;
    pub const HSKIP: u8 = 26;
    pub const VSKIP: u8 = 27;
    pub const MSKIP: u8 = 28;
    pub const KERN: u8 = 29;
    pub const MKERN: u8 = 30;
    pub const LEADER_SHIP: u8 = 31;
    pub const HALIGN: u8 = 32;
    pub const VALIGN: u8 = 33;
    pub const NO_ALIGN: u8 = 34;
    pub const VRULE: u8 = 35;
    pub const HRULE: u8 = 36;
    pub const INSERT: u8 = 37;
    pub const VADJUST: u8 = 38;
    pub const IGNORE_SPACES: u8 = 39;
    pub const AFTER_ASSIGNMENT: u8 = 40;
    pub const AFTER_GROUP: u8 = 41;
    pub const BREAK_PENALTY: u8 = 42;
    pub const START_PAR: u8 = 43;
    pub const ITAL_CORR: u8 = 44;
    pub const ACCENT: u8 = 45;
    pub const MATH_ACCENT: u8 = 46;
    pub const DISCRETIONARY: u8 = 47;
    pub const EQ_NO: u8 = 48;
    pub const LEFT_RIGHT: u8 = 49;
    pub const MATH_COMP: u8 = 50;
    pub const LIMIT_SWITCH: u8 = 51;
    pub const ABOVE: u8 = 52;
    pub const MATH_STYLE: u8 = 53;
    pub const MATH_CHOICE: u8 = 54;
    pub const NON_SCRIPT: u8 = 55;
    pub const VCENTER: u8 = 56;
    pub const CASE_SHIFT: u8 = 57;
    pub const MESSAGE: u8 = 58;
    pub const EXTENSION: u8 = 59;
    pub const IN_STREAM: u8 = 60;
    pub const BEGIN_GROUP: u8 = 61;
    pub const END_GROUP: u8 = 62;
    pub const OMIT: u8 = 63;
    pub const EX_SPACE: u8 = 64;
    pub const NO_BOUNDARY: u8 = 65;
    pub const RADICAL: u8 = 66;
    pub const END_CS_NAME: u8 = 67;
    pub const CHAR_GIVEN: u8 = 68;
    pub const MATH_GIVEN: u8 = 69;
    pub const LAST_ITEM: u8 = 70;
    pub const MAX_NON_PREFIXED: u8 = 70;
    pub const TOKS_REGISTER: u8 = 71;
    pub const ASSIGN_TOKS: u8 = 72;
    pub const ASSIGN_INT: u8 = 73;
    pub const ASSIGN_DIMEN: u8 = 74;
    pub const ASSIGN_GLUE: u8 = 75;
    pub const ASSIGN_MU_GLUE: u8 = 76;
    pub const ASSIGN_FONT_DIMEN: u8 = 77;
    pub const ASSIGN_FONT_INT: u8 = 78;
    pub const SET_AUX: u8 = 79;
    pub const SET_PREV_GRAF: u8 = 80;
    pub const SET_PAGE_DIMEN: u8 = 81;
    pub const SET_PAGE_INT: u8 = 82;
    pub const SET_BOX_DIMEN: u8 = 83;
    pub const SET_SHAPE: u8 = 84;
    pub const DEF_CODE: u8 = 85;
    pub const DEF_FAMILY: u8 = 86;
    pub const SET_FONT: u8 = 87;
    pub const DEF_FONT: u8 = 88;
    pub const REGISTER: u8 = 89;
    pub const MAX_INTERNAL: u8 = 89;
    pub const ADVANCE: u8 = 90;
    pub const MULTIPLY: u8 = 91;
    pub const DIVIDE: u8 = 92;
    pub const PREFIX: u8 = 93;
    pub const LET: u8 = 94;
    pub const SHORTHAND_DEF: u8 = 95;
    pub const READ_TO_CS: u8 = 96;
    pub const DEF: u8 = 97;
    pub const SET_BOX: u8 = 98;
    pub const HYPH_DATA: u8 = 99;
    pub const SET_INTERACTION: u8 = 100;
    pub const MAX_COMMAND: u8 = 100;
    pub const UNDEFINED_CS: u8 = 101;
    pub const EXPAND_AFTER: u8 = 102;
    pub const NO_EXPAND: u8 = 103;
    pub const INPUT: u8 = 104;
    pub const IF_TEST: u8 = 105;
    pub const FI_OR_ELSE: u8 = 106;
    pub const CS_NAME: u8 = 107;
    pub const CONVERT: u8 = 108;
    pub const THE: u8 = 109;
    pub const TOP_BOT_MARK: u8 = 110;
    pub const CALL: u8 = 111;
    pub const LONG_CALL: u8 = 112;
    pub const OUTER_CALL: u8 = 113;
    pub const LONG_OUTER_CALL: u8 = 114;
    pub const END_TEMPLATE: u8 = 115;
    pub const DONT_EXPAND: u8 = 116;
    /// A deferred read's symbol (`crate::defer`): expandable, forced or
    /// resolved by `expand`, kept as a protected macro is in a list built
    /// with expansion; past [`END_TEMPLATE`], so every test of that range
    /// names it.
    pub const SYM: u8 = 117;
    /// The command of the halt sentinel ([`crate::run::Run::raise`]): no
    /// handler matches it; a stored one is a bug.
    pub const HALT: u8 = 255;
}

// eqtb regions (§222–§236), as tex.web numbers them.
pub(crate) const SKIP_BASE: i32 = 2900;
pub(crate) const MU_SKIP_BASE: i32 = 3156;
pub(crate) const OUTPUT_ROUTINE_LOC: i32 = 3413;
pub(crate) const TOKS_BASE: i32 = 3422;
pub(crate) const MATH_FONT_BASE: i32 = 3935;
pub(crate) const CAT_CODE_BASE: i32 = 3983;
pub(crate) const LC_CODE_BASE: i32 = 4239;
pub(crate) const UC_CODE_BASE: i32 = 4495;
pub(crate) const SF_CODE_BASE: i32 = 4751;
pub(crate) const MATH_CODE_BASE: i32 = 5007;
pub(crate) const DEL_CODE_BASE: i32 = 5574;
pub(crate) const COUNT_BASE: i32 = 5318;
pub(crate) const SCALED_BASE: i32 = 5851;
/// `no_expand_flag` (§358).
pub(crate) const NO_EXPAND_FLAG: i32 = 257;

/// Group codes (§269).
#[allow(dead_code)]
pub(crate) mod group {
    pub const BOTTOM: i64 = 0;
    pub const SIMPLE: i64 = 1;
    pub const HBOX: i64 = 2;
    pub const ADJUSTED_HBOX: i64 = 3;
    pub const VBOX: i64 = 4;
    pub const VTOP: i64 = 5;
    pub const ALIGN: i64 = 6;
    pub const NO_ALIGN: i64 = 7;
    pub const OUTPUT: i64 = 8;
    pub const MATH: i64 = 9;
    pub const DISC: i64 = 10;
    pub const INSERT: i64 = 11;
    pub const VCENTER: i64 = 12;
    pub const MATH_CHOICE: i64 = 13;
    pub const SEMI_SIMPLE: i64 = 14;
    pub const MATH_SHIFT: i64 = 15;
    pub const MATH_LEFT: i64 = 16;
}

/// An interned control sequence name (`id_lookup`, §259): a number,
/// dense from 0 in order of first sight, and the name. Equal names have
/// one entry for the life of the process, so comparing and hashing a
/// control sequence touch the number only, a token is the number
/// ([`Tok`]), and a run indexes the meanings it looks up by it
/// ([`CsId::index`]). The table holds names only: ids never reach a
/// value (tokens encode as their names), so cached formats and the
/// value table do not depend on the order names were seen in.
pub(crate) struct CsEntry {
    id: u32,
    name: Box<str>,
}

/// The frozen and internal names (`\u{10FFFE}…`) are numbered below
/// this, every other name from it up (spec-final §2.6): whether a
/// control sequence is frozen is a range test on its number.
pub(crate) const FROZEN_MAX: u32 = 1 << 15;

/// Whether an internal name was numbered past [`FROZEN_MAX`] (more of
/// them than it leaves room for): then [`Tok::is_internal_cs`] reads the
/// name.
static INTERNAL_PAST_FROZEN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// The entries by number, in chunks made once (read without a lock).
const CS_CHUNK_BITS: usize = 12;
const CS_CHUNK: usize = 1 << CS_CHUNK_BITS;
const CS_CHUNKS: usize = 1 << 12;
type CsChunk = [std::sync::OnceLock<&'static CsEntry>];
static CS_BY_ID: [OnceLock<&'static CsChunk>; CS_CHUNKS] = [const { OnceLock::new() }; CS_CHUNKS];

/// A control sequence: its interned entry ([`CsEntry`]).
#[derive(Clone, Copy)]
pub(crate) struct CsId(&'static CsEntry);

impl CsId {
    /// The entry of `name`, made on its first sight.
    pub(crate) fn intern(name: &str) -> CsId {
        thread_local! {
            static LOCAL: std::cell::RefCell<std::collections::HashMap<Box<str>, CsId, std::hash::BuildHasherDefault<phitex_defuse::FxHasher>>> =
                std::cell::RefCell::default();
        }
        /// The names by text, and how many frozen and other ones there are.
        type Table = (FxHashMap<&'static str, CsId>, u32, u32);
        static TABLE: OnceLock<std::sync::Mutex<Table>> = OnceLock::new();
        if let Some(c) = LOCAL.with(|l| l.borrow().get(name).copied()) {
            crate::prof::caches::hit(crate::prof::caches::CS);
            return c;
        }
        let m = crate::prof::caches::miss(crate::prof::caches::CS);
        let c = {
            let mut t = TABLE.get_or_init(Default::default).locked();
            if let Some(&c) = t.0.get(name) {
                c
            } else {
                // (the name's class is read once, here, as it is first
                // seen: its number says it from then on)
                let id = if name.starts_with('\u{10FFFE}') && t.1 < FROZEN_MAX {
                    t.1 += 1;
                    t.1 - 1
                } else {
                    if name.starts_with('\u{10FFFE}') {
                        INTERNAL_PAST_FROZEN.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    t.2 += 1;
                    FROZEN_MAX + t.2 - 1
                };
                let e: &'static CsEntry = Box::leak(Box::new(CsEntry {
                    id,
                    name: Box::from(name),
                }));
                let id = id as usize;
                let chunk = CS_BY_ID[(id >> CS_CHUNK_BITS) % CS_CHUNKS].get_or_init(|| {
                    Box::leak(
                        (0..CS_CHUNK)
                            .map(|_| OnceLock::new())
                            .collect::<Vec<_>>()
                            .into_boxed_slice(),
                    )
                });
                let _ = chunk[id & (CS_CHUNK - 1)].set(e);
                let c = CsId(e);
                t.0.insert(&e.name, c);
                c
            }
        };
        LOCAL.with(|l| l.borrow_mut().insert(Box::from(name), c));
        crate::prof::caches::made(m);
        c
    }

    /// The control sequence numbered `i` ([`CsId::index`]).
    #[inline]
    pub(crate) fn from_index(i: usize) -> CsId {
        let e = CS_BY_ID[(i >> CS_CHUNK_BITS) % CS_CHUNKS]
            .get()
            .and_then(|c| c[i & (CS_CHUNK - 1)].get())
            .copied();
        match e {
            Some(e) => CsId(e),
            // (never: a number is made with its entry)
            None => CsId::intern(""),
        }
    }

    /// The dense number of this name.
    #[inline]
    pub(crate) fn index(self) -> usize {
        self.0.id as usize
    }

    /// Whether the name is a frozen or internal one (`\u{10FFFE}…`):
    /// its number is below [`FROZEN_MAX`].
    #[inline]
    pub(crate) fn frozen(self) -> bool {
        self.0.id < FROZEN_MAX
    }

    pub(crate) fn name(self) -> &'static str {
        &self.0.name
    }
}

impl PartialEq for CsId {
    fn eq(&self, o: &CsId) -> bool {
        self.0.id == o.0.id
    }
}

impl Eq for CsId {}

impl std::hash::Hash for CsId {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.0.id.hash(h);
    }
}

impl std::ops::Deref for CsId {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0.name
    }
}

impl std::fmt::Debug for CsId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.name(), f)
    }
}

impl std::fmt::Display for CsId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The bit of a control sequence token.
const CS_BIT: u32 = 1 << 31;

/// A token (§289) in one word: a character, `cat << 21 | char` (category
/// 13 is an active character), or, with bit 31 set, a control sequence
/// by its number ([`CsId::index`]; frozen ones' names start with
/// `\u{10FFFE}`).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub(crate) struct Tok(u32);

impl std::fmt::Debug for Tok {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.as_cs() {
            Some(n) => write!(f, "S({n:?})"),
            None => write!(f, "C({:#x})", self.0),
        }
    }
}

impl Tok {
    pub(crate) fn ch(cat: u8, c: u32) -> Tok {
        Tok((u32::from(cat) << 21 | c) & !CS_BIT)
    }

    /// The character token `x` (`cat << 21 | char`).
    pub(crate) fn c(x: u32) -> Tok {
        Tok(x & !CS_BIT)
    }

    pub(crate) fn cs(name: &str) -> Tok {
        Tok::of_cs(CsId::intern(name))
    }

    pub(crate) fn of_cs(c: CsId) -> Tok {
        Tok(CS_BIT | u32::try_from(c.index()).unwrap_or(0))
    }

    /// The word: `cat << 21 | char` for a character (so `raw() >> 21` is
    /// the category, and at least 1024 for a control sequence).
    #[inline]
    pub(crate) fn raw(self) -> u32 {
        self.0
    }

    /// `cat << 21 | char` of a character token.
    #[inline]
    pub(crate) fn as_char(self) -> Option<u32> {
        (self.0 & CS_BIT == 0).then_some(self.0)
    }

    /// The control sequence of a control sequence token.
    #[inline]
    pub(crate) fn as_cs(self) -> Option<CsId> {
        (self.0 & CS_BIT != 0).then(|| CsId::from_index((self.0 & !CS_BIT) as usize))
    }

    /// The number of a control sequence token ([`CsId::index`]).
    #[inline]
    pub(crate) fn cs_index(self) -> Option<usize> {
        (self.0 & CS_BIT != 0).then_some((self.0 & !CS_BIT) as usize)
    }

    #[inline]
    pub(crate) fn is_cs(self) -> bool {
        self.0 & CS_BIT != 0
    }

    /// Whether this is a frozen or internal control sequence: a range
    /// test on its number ([`FROZEN_MAX`]), no entry lookup.
    #[inline]
    pub(crate) fn is_frozen_cs(self) -> bool {
        self.0 & CS_BIT != 0 && self.0 & !CS_BIT < FROZEN_MAX
    }

    /// Whether this is a control sequence with an internal name
    /// (`\u{10FFFE}…`): its number below [`FROZEN_MAX`], the name read
    /// only where internal names were numbered past it.
    #[inline]
    pub(crate) fn is_internal_cs(self) -> bool {
        self.is_frozen_cs()
            || (INTERNAL_PAST_FROZEN.load(std::sync::atomic::Ordering::Relaxed)
                && self.as_cs().is_some_and(|n| n.starts_with('\u{10FFFE}')))
    }

    /// The category of a character token.
    #[inline]
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub(crate) fn cat(&self) -> Option<u8> {
        (self.0 & CS_BIT == 0).then(|| u8::try_from(self.0 >> 21).unwrap_or(0))
    }

    /// The character code of a character token.
    #[inline]
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub(crate) fn chr(&self) -> u32 {
        if self.0 & CS_BIT == 0 {
            self.0 & 0x1F_FFFF
        } else {
            0
        }
    }

    #[inline]
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub(crate) fn is_cs_or_active(&self) -> bool {
        self.0 & CS_BIT != 0 || self.0 >> 21 == 13
    }

    /// The field holding this token's meaning, for control sequences and
    /// active characters.
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub(crate) fn field_name(&self) -> Option<String> {
        match self.as_cs() {
            Some(n) => {
                // (built, not formatted: a lookup's name, made often)
                let n = n.name();
                let mut s = String::with_capacity(3 + n.len());
                s.push_str("cs:");
                s.push_str(n);
                Some(s)
            }
            None if self.cat() == Some(13) => Some(format!("act:{}", self.chr())),
            None => None,
        }
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub(crate) fn encode(&self, v: &Values) -> ValueId {
        match self.as_cs() {
            None => v.int(i64::from(self.0)),
            Some(n) => {
                let n = n.name();
                let mut s = String::with_capacity(1 + n.len());
                s.push('\\');
                s.push_str(n);
                v.str(&s)
            }
        }
    }

    pub(crate) fn decode(v: &Values, id: ValueId) -> Tok {
        if let Some(i) = v.as_int(id) {
            return Tok::c(u32::try_from(i).unwrap_or(0));
        }
        v.with_str(id, |s| Tok::cs(s.strip_prefix('\\').unwrap_or(s)))
            .unwrap_or_else(|| Tok::c(0))
    }
}

/// The space token `␣₁₀` (§289).
pub(crate) fn space_tok() -> Tok {
    Tok::ch(10, 32)
}

pub(crate) fn toks_value(v: &Values, toks: &[Tok]) -> ValueId {
    v.list(toks.iter().map(|t| tok_value(v, *t)).collect())
}

/// The value of token `t`: in a run, through the worker's cache
/// ([`tok_id`] at the run's epoch, [`EpochScope`]).
fn tok_value(v: &Values, t: Tok) -> ValueId {
    match scope_epoch(v) {
        Some(e) => tok_id(v, e, t),
        None => t.encode(v),
    }
}

pub(crate) fn toks_of(v: &Values, id: Option<ValueId>) -> Vec<Tok> {
    toks_rc(v, id).to_vec()
}

/// The token list `id`, shared with this thread's cache (no copy).
pub(crate) fn toks_rc(v: &Values, id: Option<ValueId>) -> Rc<[Tok]> {
    match id {
        Some(id) => decoded(
            v,
            id,
            |d| &mut d.toks,
            || {
                v.with_list(id, |xs| xs.iter().map(|&x| Tok::decode(v, x)).collect())
                    .unwrap_or_else(|| Rc::from([]))
            },
        ),
        None => Rc::from([]),
    }
}

/// Each thread's decoded values, by id: an id is a content hash, so a
/// decoding never goes stale, and a value being decoded is live (a
/// swept id is never asked for again). Saves the value reads of a token
/// list or meaning read again by later runs on the same worker.
#[derive(Default)]
struct Decoded {
    instance: u64,
    toks: FxHashMap<ValueId, Rc<[Tok]>>,
    means: FxHashMap<ValueId, Mean>,
    glues: FxHashMap<ValueId, Glue>,
    frames: FxHashMap<ValueId, Option<crate::input::Seg>>,
    leaves: FxHashMap<ValueId, phitex_typeset::node::Node>,
}

/// Entries kept per map before it is emptied (a bound, not a policy).
const DECODED_MAX: usize = 1 << 15;

thread_local! {
    static DECODED: std::cell::RefCell<Decoded> = std::cell::RefCell::default();
    static ENCODED: std::cell::RefCell<Encoded> = std::cell::RefCell::default();
}

/// Each thread's encoded values at one values epoch of one table: an id
/// made at an epoch is valid while it lasts
/// ([`phitex_defuse::Ctx::values_epoch`]), so a token written again by
/// a later run on the same worker is interned once an epoch, not once a
/// write (the leftover of every run encodes its tokens).
#[derive(Default)]
struct Encoded {
    instance: u64,
    epoch: u64,
    toks: FxHashMap<Tok, ValueId>,
    ints: FxHashMap<i64, ValueId>,
    strs: FxHashMap<Box<str>, ValueId>,
    glues: FxHashMap<Glue, ValueId>,
    means: FxHashMap<MeanKey, ValueId>,
    /// Macros by address (the `Rc` kept, so an address is never reused
    /// while it is a key): a `\\let` of a macro encodes nothing.
    macros: FxHashMap<usize, (Rc<Macro>, ValueId)>,
    nodes: FxHashMap<NodeKey, ValueId>,
    /// Character nodes, direct-mapped by `(font, character)`: the key
    /// plus one (0: empty) and the value.
    chars: Vec<(u64, ValueId)>,
}

/// The size of [`Encoded::chars`].
const CHAR_NODES: usize = 1 << 12;

/// The value of the character node `(font, ch)` (`make()` builds it),
/// cached in a run.
pub(crate) fn char_node_id(
    v: &Values,
    font: u32,
    ch: u16,
    make: impl FnOnce() -> ValueId,
) -> ValueId {
    let Some(epoch) = scope_epoch(v) else {
        return make();
    };
    let key = (u64::from(font) << 16 | u64::from(ch)) + 1;
    let slot = (usize::from(ch) ^ (font as usize).wrapping_mul(0x9E37)) & (CHAR_NODES - 1);
    let hit = ENCODED.with(|c| {
        let mut c = c.borrow_mut();
        if c.instance != v.epoch() || c.epoch != epoch {
            crate::prof::caches::hit(crate::prof::caches::ENC_RESET);
            *c = Encoded {
                instance: v.epoch(),
                epoch,
                ..Encoded::default()
            };
        }
        if c.chars.is_empty() {
            c.chars = vec![(0, ValueId::UNIT); CHAR_NODES];
        }
        let (k, x) = c.chars[slot];
        (k == key).then_some(x)
    });
    if let Some(x) = hit {
        crate::prof::caches::hit(crate::prof::caches::ENC_CHAR);
        return x;
    }
    let m = crate::prof::caches::miss(crate::prof::caches::ENC_CHAR);
    let x = make();
    crate::prof::caches::made(m);
    ENCODED.with(|c| {
        if let Some(e) = c.borrow_mut().chars.get_mut(slot) {
            *e = (key, x);
        }
    });
    x
}

/// A node of fixed shape, as the key of its value: its tag, up to three
/// integers and a glue.
pub(crate) type NodeKey = (i64, [i64; 3], Option<Glue>);

/// A meaning other than a macro, as the key of its value.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum MeanKey {
    Prim(&'static str),
    Char(u8, u32),
    Undefined,
    Font(i64),
    CharDef(i64),
    MathCharDef(i64),
    Reg(u8, i32),
    NotExpanded,
}

thread_local! {
    /// The values table and epoch of the run on this thread, if one is
    /// running ([`EpochScope`]).
    static SCOPE: std::cell::Cell<Option<(u64, u64)>> = const { std::cell::Cell::new(None) };
}

/// While alive, encodings on this thread go through the worker's cache
/// at `epoch` (a run holds one: an id made at an epoch is valid while it
/// lasts, and a run runs on one thread from start to finish).
pub(crate) struct EpochScope {
    prev: Option<(u64, u64)>,
}

impl EpochScope {
    pub(crate) fn enter(v: &Values, epoch: u64) -> EpochScope {
        EpochScope {
            prev: SCOPE.with(|c| c.replace(Some((v.instance(), epoch)))),
        }
    }
}

impl Drop for EpochScope {
    fn drop(&mut self) {
        SCOPE.with(|c| c.set(self.prev));
    }
}

/// The epoch of the run on this thread, if it uses `v`.
fn scope_epoch(v: &Values) -> Option<u64> {
    SCOPE
        .with(std::cell::Cell::get)
        .and_then(|(i, e)| (i == v.instance()).then_some(e))
}

/// `make()`, the value keyed `key` in `map`, through this thread's cache
/// when a run is in scope; else made.
fn encoded<K: std::hash::Hash + Eq>(
    v: &Values,
    map: fn(&mut Encoded) -> &mut FxHashMap<K, ValueId>,
    key: K,
    make: impl FnOnce() -> ValueId,
) -> ValueId {
    let Some(epoch) = scope_epoch(v) else {
        return make();
    };
    let hit = ENCODED.with(|c| {
        let mut c = c.borrow_mut();
        if c.instance != v.epoch() || c.epoch != epoch {
            crate::prof::caches::hit(crate::prof::caches::ENC_RESET);
            *c = Encoded {
                instance: v.epoch(),
                epoch,
                ..Encoded::default()
            };
        }
        map(&mut c).get(&key).copied()
    });
    if let Some(x) = hit {
        crate::prof::caches::hit(crate::prof::caches::ENC);
        return x;
    }
    let m = crate::prof::caches::miss(crate::prof::caches::ENC);
    let x = make();
    crate::prof::caches::made(m);
    ENCODED.with(|c| {
        let mut c = c.borrow_mut();
        let m = map(&mut c);
        if m.len() >= DECODED_MAX {
            m.clear();
        }
        m.insert(key, x);
    });
    x
}

/// The value of integer `i` (cached in a run).
pub(crate) fn int_id(v: &Values, i: i64) -> ValueId {
    // (an integer of 60 bits is its id: no table, no cache)
    if (-(1_i64 << 59)..(1_i64 << 59)).contains(&i) {
        return v.int(i);
    }
    encoded(v, |c| &mut c.ints, i, || v.int(i))
}

/// The value of string `s` (cached in a run).
pub(crate) fn str_id(v: &Values, s: &str) -> ValueId {
    if let Some(e) = scope_epoch(v) {
        let hit = ENCODED.with(|c| {
            let c = c.borrow();
            (c.instance == v.epoch() && c.epoch == e)
                .then(|| c.strs.get(s).copied())
                .flatten()
        });
        if let Some(x) = hit {
            crate::prof::caches::hit(crate::prof::caches::ENC);
            return x;
        }
    }
    encoded(v, |c| &mut c.strs, Box::from(s), || v.str(s))
}

/// The value of the fixed-shape node `key` (`make()` builds it), cached
/// in a run.
pub(crate) fn node_id(v: &Values, key: NodeKey, make: impl FnOnce() -> ValueId) -> ValueId {
    encoded(v, |c| &mut c.nodes, key, make)
}

/// The value of token `t` ([`Tok::encode`]) in a run at `epoch`, through
/// this thread's cache.
pub(crate) fn tok_id(v: &Values, epoch: u64, t: Tok) -> ValueId {
    let hit = ENCODED.with(|c| {
        let mut c = c.borrow_mut();
        if c.instance != v.epoch() || c.epoch != epoch {
            crate::prof::caches::hit(crate::prof::caches::ENC_RESET);
            *c = Encoded {
                instance: v.epoch(),
                epoch,
                ..Encoded::default()
            };
        }
        c.toks.get(&t).copied()
    });
    if let Some(x) = hit {
        crate::prof::caches::hit(crate::prof::caches::ENC_TOK);
        return x;
    }
    let m = crate::prof::caches::miss(crate::prof::caches::ENC_TOK);
    let x = t.encode(v);
    crate::prof::caches::made(m);
    ENCODED.with(|c| {
        let mut c = c.borrow_mut();
        if c.toks.len() >= DECODED_MAX {
            c.toks.clear();
        }
        c.toks.insert(t, x);
    });
    x
}

/// The input frame `id` (`make()` decodes it), through this thread's
/// cache.
pub(crate) fn frame_of(
    v: &Values,
    id: ValueId,
    make: impl FnOnce() -> Option<crate::input::Seg>,
) -> Option<crate::input::Seg> {
    decoded(v, id, |d| &mut d.frames, make)
}

/// The node `id` decoded before on this thread, if its decoding met no
/// mark or whatsit (then it is a function of its value alone: a box and
/// its list included).
pub(crate) fn leaf_of(v: &Values, id: ValueId) -> Option<phitex_typeset::node::Node> {
    let r = leaf_of_cached(v, id);
    if r.is_some() {
        crate::prof::caches::hit(crate::prof::caches::LEAF);
    } else {
        crate::prof::caches::made(crate::prof::caches::miss(crate::prof::caches::LEAF));
    }
    r
}

fn leaf_of_cached(v: &Values, id: ValueId) -> Option<phitex_typeset::node::Node> {
    DECODED.with(|c| {
        let c = c.borrow();
        (c.instance == v.epoch())
            .then(|| c.leaves.get(&id).cloned())
            .flatten()
    })
}

thread_local! {
    /// Set while a pass decodes values it will not decode again (the
    /// DVI writer's pages): their leaves are not kept.
    pub(crate) static NO_KEEP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Keeps leaf node `n`, the decoding of `id` ([`leaf_of`]).
pub(crate) fn keep_leaf(v: &Values, id: ValueId, n: &phitex_typeset::node::Node) {
    if NO_KEEP.with(std::cell::Cell::get) {
        return;
    }
    DECODED.with(|c| {
        let mut c = c.borrow_mut();
        if c.instance != v.epoch() {
            *c = Decoded {
                instance: v.epoch(),
                ..Decoded::default()
            };
        }
        if c.leaves.len() >= DECODED_MAX {
            c.leaves.clear();
        }
        c.leaves.insert(id, n.clone());
    });
}

/// Character nodes decoded on a thread, direct-mapped by id: the values
/// table's instance, then `(id, font, character)` (an id is a content
/// hash: its decoding never goes stale).
type CharNodes = (u64, Vec<(u64, u32, u16)>);

thread_local! {
    static CHAR_NODES_DEC: std::cell::RefCell<CharNodes> =
        const { std::cell::RefCell::new((u64::MAX, Vec::new())) };
}

const CHAR_NODES_DEC_SIZE: usize = 1 << 12;

/// The character node `id` decoded before on this thread, if it is one.
pub(crate) fn char_node_of(v: &Values, id: ValueId) -> Option<(u32, u16)> {
    let r = char_node_of_cached(v, id);
    if r.is_some() {
        crate::prof::caches::hit(crate::prof::caches::DEC_CHAR);
    } else {
        crate::prof::caches::made(crate::prof::caches::miss(crate::prof::caches::DEC_CHAR));
    }
    r
}

fn char_node_of_cached(v: &Values, id: ValueId) -> Option<(u32, u16)> {
    let raw = id.raw();
    CHAR_NODES_DEC.with(|c| {
        let c = c.borrow();
        if c.0 != v.epoch() {
            return None;
        }
        #[allow(clippy::cast_possible_truncation)]
        let (k, f, ch) =
            *c.1.get((raw ^ (raw >> 17)) as usize & (CHAR_NODES_DEC_SIZE - 1))?;
        (k == raw).then_some((f, ch))
    })
}

/// Keeps `id`'s decoding as the character node `(font, ch)`.
pub(crate) fn keep_char_node(v: &Values, id: ValueId, font: u32, ch: u16) {
    let raw = id.raw();
    CHAR_NODES_DEC.with(|c| {
        let mut c = c.borrow_mut();
        if c.0 != v.epoch() || c.1.is_empty() {
            *c = (v.epoch(), vec![(u64::MAX, 0, 0); CHAR_NODES_DEC_SIZE]);
        }
        #[allow(clippy::cast_possible_truncation)]
        let k = (raw ^ (raw >> 17)) as usize & (CHAR_NODES_DEC_SIZE - 1);
        c.1[k] = (raw, font, ch);
    });
}

/// `make()`, the decoding of `id`, through this thread's cache.
fn decoded<T: Clone>(
    v: &Values,
    id: ValueId,
    map: fn(&mut Decoded) -> &mut FxHashMap<ValueId, T>,
    make: impl FnOnce() -> T,
) -> T {
    let hit = DECODED.with(|c| {
        let mut c = c.borrow_mut();
        if c.instance != v.epoch() {
            *c = Decoded {
                instance: v.epoch(),
                ..Decoded::default()
            };
        }
        map(&mut c).get(&id).cloned()
    });
    if let Some(x) = hit {
        crate::prof::caches::hit(crate::prof::caches::DEC);
        return x;
    }
    let m = crate::prof::caches::miss(crate::prof::caches::DEC);
    let x = make();
    crate::prof::caches::made(m);
    DECODED.with(|c| {
        let mut c = c.borrow_mut();
        let m = map(&mut c);
        if m.len() >= DECODED_MAX {
            m.clear();
        }
        m.insert(id, x.clone());
    });
    x
}

pub(crate) fn list_of(v: &Values, id: ValueId) -> Vec<ValueId> {
    // (borrowed: no `Arc` of the value per call)
    v.with(id, |x| match x {
        Value::List(xs) => Some(xs.to_vec()),
        Value::Seq(_) | Value::Nil => None,
        _ => Some(Vec::new()),
    })
    .unwrap_or_else(|| v.seq_to_vec(id))
}

pub(crate) fn int_of(v: &Values, id: ValueId) -> i64 {
    v.as_int(id).unwrap_or(0)
}

pub(crate) fn int_at(v: &Values, id: ValueId, k: usize) -> i64 {
    v.with_list(id, |xs| xs.get(k).map_or(0, |&x| int_of(v, x)))
        .unwrap_or(0)
}

pub(crate) fn str_of(v: &Values, id: ValueId) -> String {
    v.with_str(id, str::to_owned).unwrap_or_default()
}

pub(crate) fn field_name(v: &Values, f: Field) -> String {
    str_of(v, f.0)
}

/// A definition's value shown briefly (`PHITEX_AUX_TRACE`): a macro as
/// its body, anything else as its tree, cut at 200 characters.
pub(crate) fn show_def(v: &Values, x: ValueId) -> String {
    let mut s = String::new();
    if let Mean::Macro(m) = Mean::decode(v, Some(x)) {
        s.push_str("macro:");
        crate::print::show_toks(&mut s, &m.body, 92, &mut |_| 11);
    } else {
        use std::fmt::Write as _;
        let _ = write!(s, "{:?}", v.get(x));
    }
    if s.len() > 200 {
        let mut k = 200;
        while !s.is_char_boundary(k) {
            k -= 1;
        }
        s.truncate(k);
        s.push('…');
    }
    s
}

/// The key [`clist_projection`] answers for a meaning that is no plain
/// comma list.
pub(crate) const CLIST_NOT: &str = "\u{1}not a list";

/// A parameterless macro's body as the items of a comma list, for a
/// membership test (`[[item]…]`, each item its tokens between commas at
/// brace depth 0, `[[CLIST_NOT]]` for any other meaning, or a body
/// holding a brace, `\\@empty` or `\\@nnil`, where a delimited match is
/// more than membership): what microtype's `\\MT@in@clist{x}\\list` reads
/// of `\\list` (`Run::clist_has`). A pure projection for the engine: a
/// list grown by an item a reader does not test for leaves it standing.
pub(crate) fn clist_projection(x: ValueId, v: &Values) -> ValueId {
    // (kept per value: every reader of a list the engine asks again
    // answers through it, a whole list decoded each time otherwise)
    static MEMO: OnceLock<crate::hints::Shards<(u64, ValueId, ValueId)>> = OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    let k = x.raw() ^ v.epoch().rotate_left(32);
    if let Some(&(i, y, r)) = memo.of(k).get(&k)
        && i == v.epoch()
        && y == x
    {
        return r;
    }
    let r = clist_items(x, v);
    let mut m = memo.of(k);
    if m.len() > 1 << 12 {
        m.clear();
    }
    m.insert(k, (v.epoch(), x, r));
    r
}

/// [`clist_projection`]'s answer, made.
fn clist_items(x: ValueId, v: &Values) -> ValueId {
    let not = || v.list(vec![v.list(vec![v.str(CLIST_NOT)])]);
    let Mean::Macro(m) = Mean::decode(v, Some(x)) else {
        return not();
    };
    if !m.params.is_empty() {
        return not();
    }
    let mut items = Vec::new();
    let mut cur: Vec<Tok> = Vec::new();
    for &t in m.body.iter() {
        if let Some(c) = t.as_cs() {
            if matches!(c.name(), "@empty" | "@nnil") {
                return not();
            }
        } else if matches!(t.cat(), Some(1 | 2)) {
            return not();
        } else if t.cat() == Some(12) && t.chr() == u32::from(b',') {
            items.push(v.list(vec![toks_value(v, &cur)]));
            cur.clear();
            continue;
        }
        cur.push(t);
    }
    items.push(v.list(vec![toks_value(v, &cur)]));
    v.list(items)
}

/// What an insertion class's box register is to the page builder
/// (§1008-§1009: `x` its binding `[level, box]`): whether the box is an
/// hbox and its height plus depth, none where it is void. A pure
/// projection for the engine: a box changed within that shape (a routine
/// that rebuilt it from other text of the same size) leaves a reader of
/// the class's parameters standing.
pub(crate) fn box_shape_projection(x: ValueId, v: &Values) -> ValueId {
    let b = v
        .with_list(x, |xs| xs.get(1).copied().filter(|&b| b != ValueId::UNIT))
        .flatten();
    b.map_or(ValueId::UNIT, |b| {
        let hd = crate::nodes::box_dim(v, b, 3) + crate::nodes::box_dim(v, b, 2);
        v.list(vec![
            v.int(i64::from(crate::nodes::box_is_h(v, b))),
            v.int(i64::from(hd)),
        ])
    })
}

/// The class of a meaning as a lookup that only stores its token needs
/// it (spec-redesign §8, class reads; `\edef`'s expansion, §478): the
/// command code of an unexpandable meaning, `\relax`'s for a
/// `\protected` macro that is not `\outer` (e-TeX keeps it as its
/// token), -1 where the meaning is needed in full (expandable, outer,
/// undefined). A pure projection for the engine
/// (`phitex_defuse::Engine::register_projection`): a redefinition with
/// the same class leaves a class reader standing.
pub(crate) fn class_projection(x: ValueId, v: &Values) -> ValueId {
    let m = Mean::decode(v, Some(x));
    let c = if let Mean::Macro(mm) = &m {
        if mm.flags & crate::etex::PROTECTED != 0 && mm.flags & 2 == 0 {
            i64::from(cmd::RELAX)
        } else {
            -1
        }
    } else {
        let (c, _) = m.cmd_chr();
        if c <= cmd::MAX_COMMAND {
            i64::from(c)
        } else {
            -1
        }
    };
    v.int(c)
}

/// Whether a meaning is an `\outer` macro (§336), as a lookup that only
/// steps over its token needs it (`\expandafter`'s first, §368): `get_next`
/// checks an outer one's validity, nothing else of it is used. A pure
/// projection for the engine: a redefinition that keeps a name outer, or
/// not, leaves such a reader standing.
pub(crate) fn outer_projection(x: ValueId, v: &Values) -> ValueId {
    let outer = matches!(Mean::decode(v, Some(x)), Mean::Macro(m) if m.flags & 2 != 0);
    v.int(i64::from(outer))
}

/// What a token stepped over or stored without expansion reads of its
/// meaning: a skipped conditional branch counts the conditionals it opens
/// and closes and stops at an `\else`, `\or` or `\fi` of its own level,
/// whose code its caller reads (§494); an alignment's `get_next` watches
/// for a tab mark or `\cr` (§342); a definition's parameter text and
/// body look for the macro parameter character (§476, §479); `get_next`
/// checks an `\outer` macro's validity (§336); a macro's arguments and a
/// token list's body test the token alone (§392, §473). Nothing else of
/// the meaning is used. [`STEP_IF`], [`STEP_OUTER`], [`STEP_FI`] plus the
/// `\fi`/`\else`/`\or` code, [`STEP_PARAM`] plus the character,
/// [`STEP_TAB`] or [`STEP_CR`] plus the code, or 0.
/// A pure projection for the engine: a redefinition that keeps the name in
/// its class leaves the reading run standing.
pub(crate) fn step_projection(x: ValueId, v: &Values) -> ValueId {
    let m = Mean::decode(v, Some(x));
    let c = if let Mean::Macro(mm) = &m {
        if mm.flags & 2 != 0 { STEP_OUTER } else { 0 }
    } else {
        match m.cmd_chr() {
            (cmd::IF_TEST, _) => STEP_IF,
            (cmd::FI_OR_ELSE, h) => STEP_FI + i64::from(h),
            (cmd::MAC_PARAM, h) => STEP_PARAM + i64::from(h),
            (cmd::TAB_MARK, h) => STEP_TAB + i64::from(h),
            (cmd::CAR_RET, h) => STEP_CR + i64::from(h),
            _ => 0,
        }
    };
    v.int(c)
}

/// The number a meaning gives a number scan (§440, §444): a macro
/// without parameters, not `\outer`, whose body is decimal digits
/// (category 12) making a number TeX takes without error, gives that
/// number; anything else -1. A pure projection for the engine: a
/// comparison of such a macro's number reads the comparison's outcome
/// through it (`Step::Cmp`, [`crate::run::Run::num_order`]), so a
/// redefinition that keeps the outcome leaves the reading run standing.
pub(crate) fn num_projection(x: ValueId, v: &Values) -> ValueId {
    let n = match Mean::decode(v, Some(x)) {
        Mean::Macro(m) if m.params.is_empty() && m.flags & 2 == 0 && !m.body.is_empty() => {
            let mut n: i64 = 0;
            for t in m.body.iter() {
                if t.is_cs_or_active() || t.cat() != Some(12) || !(48..=57).contains(&t.chr()) {
                    n = -1;
                    break;
                }
                n = n * 10 + i64::from(t.chr() - 48);
                if n > i64::from(i32::MAX) {
                    n = -1;
                    break;
                }
            }
            n
        }
        _ => -1,
    };
    v.int(n)
}

/// Whether a meaning is defined, as `\csname` needs it (§372): a name
/// it makes that is undefined becomes `\relax`; nothing else of the
/// meaning is used there. A pure projection for the engine: a
/// redefinition of a defined name leaves the reading run standing.
pub(crate) fn defined_projection(x: ValueId, v: &Values) -> ValueId {
    v.int(i64::from(Mean::decode(v, Some(x)) != Mean::Undefined))
}

/// What a reader of the main list's last item by its kind needs of it
/// ([`crate::build::MAIN_LAST`]: `\\lastskip`, `\\lastkern`,
/// `\\lastpenalty`, §424; `\\unskip` and its kind, §1105;
/// `\\lastnodetype`; whether `\\lastbox` takes it, §1080): a glue, kern
/// or penalty item whole, a box as its tag and whether it is horizontal
/// (`[tag, h]`, [`crate::nodes::box_is_h`]'s field), anything else as its
/// tag alone (`[tag]`); no item stays none. A pure projection for the
/// engine: a paragraph ending with another line box than the one before
/// leaves such a reader standing.
pub(crate) fn tail_projection(x: ValueId, v: &Values) -> ValueId {
    use crate::nodes::{T_BOX, T_GLUE, T_KERN, T_PENALTY};
    let Some((tag, h)) = v.with_list(x, |xs| {
        let tag = xs.first().map_or(-1, |&t| int_of(v, t));
        (tag, xs.get(1).copied())
    }) else {
        return x;
    };
    match tag {
        T_GLUE | T_KERN | T_PENALTY => x,
        T_BOX => v.list(vec![v.int(T_BOX), h.unwrap_or(ValueId::UNIT)]),
        t => v.list(vec![v.int(t)]),
    }
}

/// [`step_projection`]'s class of a meaning that opens a conditional.
pub(crate) const STEP_IF: i64 = 1;
/// [`step_projection`]'s class of an `\outer` macro.
pub(crate) const STEP_OUTER: i64 = 2;
/// [`step_projection`]'s class of `\fi`, `\else` and `\or`, plus the code.
pub(crate) const STEP_FI: i64 = 16;
/// [`step_projection`]'s class of the macro parameter character, plus the
/// character.
pub(crate) const STEP_PARAM: i64 = 1 << 32;
/// [`step_projection`]'s class of an alignment tab mark, plus its code: an
/// alignment's `get_next` inserts a template at one (§342).
pub(crate) const STEP_TAB: i64 = 2 << 32;
/// [`step_projection`]'s class of `\cr` and `\crcr`, plus the code (§342).
pub(crate) const STEP_CR: i64 = 3 << 32;

/// A meaning (§222): what a control sequence or active character is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Mean {
    Prim(&'static str, u8, i32),
    Macro(Rc<Macro>),
    Char(u8, u32),
    Undefined,
    Font(i64),
    CharDef(i64),
    MathCharDef(i64),
    /// `\countdef` etc.: the command and the eqtb location.
    Reg(u8, i32),
    /// The marker of `\noexpand` (§358).
    NotExpanded,
}

/// A macro (§210, §473): flags (1 `\long`, 2 `\outer`), its parameter
/// text (match tokens are `Tok::ch(14, char)`), its body
/// (parameters are `Tok::ch(5, n)`).
#[derive(Debug, PartialEq)]
pub(crate) struct Macro {
    pub flags: u8,
    pub params: Vec<Tok>,
    /// Shared: a call without arguments reads it in place.
    pub body: Rc<[Tok]>,
    /// The body cut at its parameters (§400): each segment is a run of
    /// plain tokens followed by one parameter (0: none). Empty when the
    /// macro has no parameters (its body is read in place).
    pub segs: Box<[Seg]>,
}

/// One piece of a macro body: `body[lo..hi]` then parameter `param`
/// (1-based; 0 for the tail without one).
#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) struct Seg {
    pub lo: u32,
    pub hi: u32,
    pub param: u8,
}

impl Macro {
    pub(crate) fn new(flags: u8, params: Vec<Tok>, body: Rc<[Tok]>) -> Self {
        let segs = if params.is_empty() {
            Box::default()
        } else {
            let mut segs = Vec::new();
            let mut lo = 0u32;
            for (i, t) in body.iter().enumerate() {
                if let Some(x) = t.as_char()
                    && x >> 21 == u32::from(OUT_PARAM_CAT)
                {
                    let i = u32::try_from(i).unwrap_or(u32::MAX);
                    segs.push(Seg {
                        lo,
                        hi: i,
                        param: u8::try_from(x & 0x1F_FFFF).unwrap_or(1),
                    });
                    lo = i + 1;
                }
            }
            let end = u32::try_from(body.len()).unwrap_or(u32::MAX);
            if lo < end {
                segs.push(Seg {
                    lo,
                    hi: end,
                    param: 0,
                });
            }
            segs.into_boxed_slice()
        };
        Macro {
            flags,
            params,
            body,
            segs,
        }
    }
}

pub(crate) const MATCH_CAT: u8 = 14;

/// `end_match` (§289), only where a macro is shown.
pub(crate) fn end_match() -> Tok {
    Tok::c(15 << 21)
}
pub(crate) const OUT_PARAM_CAT: u8 = 5;

fn prim_index() -> &'static phitex_defuse::FxHashMap<&'static str, (u8, i32)> {
    static M: OnceLock<phitex_defuse::FxHashMap<&'static str, (u8, i32)>> = OnceLock::new();
    M.get_or_init(|| {
        // (XeTeX's own names among them: a name is one primitive in
        // either engine, a meaning names it)
        crate::etex::all_prims()
            .copied()
            .chain(crate::xetex::own_prims())
            .map(|(n, c, h)| (n, (c, h)))
            .collect()
    })
}

fn prim_names() -> &'static phitex_defuse::FxHashMap<(u8, i32), &'static str> {
    static M: OnceLock<phitex_defuse::FxHashMap<(u8, i32), &'static str>> = OnceLock::new();
    M.get_or_init(|| {
        let mut m: phitex_defuse::FxHashMap<(u8, i32), &'static str> = crate::etex::all_prims()
            .map(|&(n, c, h)| ((c, h), n))
            .collect();
        // (XeTeX's own quantities, by codes no other primitive has; of
        // two names of one, the later, `\U…` over `\XeTeX…`)
        let own: Vec<_> = crate::xetex::own_prims().collect();
        for &(n, c, h) in own.iter().rev() {
            m.entry((c, h)).or_insert(n);
        }
        m
    })
}

/// The primitive named `name` (frozen and internal ones included).
pub(crate) fn prim(name: &str) -> Option<(&'static str, u8, i32)> {
    let base = name.strip_prefix('\u{10FFFE}').unwrap_or(name);
    match name {
        // internal: `end_graf` from a closing brace (§1096, §1100)
        "\u{10FFFE}endgraf" => return Some(("\u{10FFFE}endgraf", cmd::PAR_END, 1)),
        "\u{10FFFE}notexpanded" => return Some(("\u{10FFFE}notexpanded", cmd::DONT_EXPAND, 0)),
        "\u{10FFFE}endv" => return Some(("\u{10FFFE}endv", cmd::IGNORE, 0)),
        // internal: `align_peek` after a row or `\noalign` (§785)
        "\u{10FFFE}alignpeek" => return Some(("\u{10FFFE}alignpeek", cmd::NO_ALIGN, 1)),
        _ => {}
    }
    prim_index()
        .get_key_value(base)
        .map(|(&n, &(c, h))| (n, c, h))
}

/// The internal primitives' names ([`prim`]'s own).
const INTERNAL_PRIMS: [&str; 4] = [
    "\u{10FFFE}endgraf",
    "\u{10FFFE}notexpanded",
    "\u{10FFFE}endv",
    "\u{10FFFE}alignpeek",
];

/// Every primitive's name by a dense handle, and the handle by name
/// (a meaning slot keeps a primitive's name as its handle,
/// `crate::state::Entry`).
fn prim_handles() -> &'static (
    Vec<&'static str>,
    phitex_defuse::FxHashMap<&'static str, u32>,
) {
    static M: OnceLock<(
        Vec<&'static str>,
        phitex_defuse::FxHashMap<&'static str, u32>,
    )> = OnceLock::new();
    M.get_or_init(|| {
        let mut names: Vec<&'static str> = Vec::new();
        let mut ix = phitex_defuse::FxHashMap::default();
        for n in crate::etex::all_prims()
            .map(|p| p.0)
            .chain(crate::xetex::own_prims().map(|p| p.0))
            .chain(INTERNAL_PRIMS.iter().copied())
        {
            ix.entry(n).or_insert_with(|| {
                names.push(n);
                u32::try_from(names.len() - 1).unwrap_or(0)
            });
        }
        (names, ix)
    })
}

/// The handle of primitive name `n` (as [`prim`] gives names).
pub(crate) fn prim_handle(n: &'static str) -> u32 {
    prim_handles().1.get(n).copied().unwrap_or(u32::MAX)
}

/// The primitive name of handle `h` ([`prim_handle`]).
pub(crate) fn prim_by_handle(h: u32) -> &'static str {
    prim_handles().0.get(h as usize).copied().unwrap_or("relax")
}

/// The name of the parameter or primitive at `(cmd, chr)`.
pub(crate) fn prim_name(c: u8, chr: i32) -> Option<&'static str> {
    prim_names().get(&(c, chr)).copied()
}

impl Mean {
    pub(crate) fn cmd_chr(&self) -> (u8, i32) {
        match self {
            Mean::Prim(_, c, h) | Mean::Reg(c, h) => (*c, *h),
            Mean::Macro(m) => (cmd::CALL + (m.flags & 3), 0),
            Mean::Char(c, h) => (*c, i32::try_from(*h).unwrap_or(0)),
            Mean::Undefined => (cmd::UNDEFINED_CS, 0),
            Mean::Font(f) => (cmd::SET_FONT, i32::try_from(*f).unwrap_or(0)),
            Mean::CharDef(n) => (cmd::CHAR_GIVEN, i32::try_from(*n).unwrap_or(0)),
            Mean::MathCharDef(n) if *n >= crate::xetex::U_MATH => (
                cmd::MATH_GIVEN,
                u32::try_from(*n & 0xFFFF_FFFF).unwrap_or(0).cast_signed(),
            ),
            Mean::MathCharDef(n) => (cmd::MATH_GIVEN, i32::try_from(*n).unwrap_or(0)),
            Mean::NotExpanded => (cmd::RELAX, NO_EXPAND_FLAG),
        }
    }

    pub(crate) fn encode(&self, v: &Values) -> ValueId {
        let key = match self {
            Mean::Macro(m) => return macro_id(v, m),
            Mean::Prim(n, _, _) => MeanKey::Prim(n),
            Mean::Char(c, h) => MeanKey::Char(*c, *h),
            Mean::Undefined => MeanKey::Undefined,
            Mean::Font(f) => MeanKey::Font(*f),
            Mean::CharDef(n) => MeanKey::CharDef(*n),
            Mean::MathCharDef(n) => MeanKey::MathCharDef(*n),
            Mean::Reg(c, h) => MeanKey::Reg(*c, *h),
            Mean::NotExpanded => MeanKey::NotExpanded,
        };
        encoded(v, |c| &mut c.means, key, || self.encode_new(v))
    }

    fn encode_new(&self, v: &Values) -> ValueId {
        let s = |x: &str| str_id(v, x);
        let i = |x: i64| int_id(v, x);
        match self {
            Mean::Prim(n, _, _) => v.list(vec![s("prim"), s(n)]),
            Mean::Macro(m) => v.list(vec![
                s("macro"),
                i(i64::from(m.flags)),
                toks_value(v, &m.params),
                toks_value(v, &m.body),
            ]),
            Mean::Char(c, h) => v.list(vec![s("char"), i(i64::from(*c)), i(i64::from(*h))]),
            Mean::Undefined => v.list(vec![s("undefined")]),
            Mean::Font(f) => v.list(vec![s("font"), i(*f)]),
            Mean::CharDef(n) => v.list(vec![s("chardef"), i(*n)]),
            Mean::MathCharDef(n) => v.list(vec![s("mathchardef"), i(*n)]),
            Mean::Reg(c, h) => {
                let (tag, base) = reg_tag(*c);
                v.list(vec![s(tag), i(i64::from(*h - base))])
            }
            Mean::NotExpanded => v.list(vec![s("notexpanded")]),
        }
    }

    pub(crate) fn decode(v: &Values, id: Option<ValueId>) -> Mean {
        let Some(id) = id else { return Mean::Undefined };
        decoded(
            v,
            id,
            |d| &mut d.means,
            || {
                let m = Mean::decode_new(v, id);
                if let Mean::Macro(x) = &m {
                    remember_macro(v, x, id);
                }
                m
            },
        )
    }

    fn decode_new(v: &Values, id: ValueId) -> Mean {
        let Some(xs) = v.with_list(id, <[ValueId]>::to_vec) else {
            return Mean::Undefined;
        };
        let Some(&t) = xs.first() else {
            return Mean::Undefined;
        };
        let tag = str_of(v, t);
        let n = |k: usize| xs.get(k).map_or(0, |&x| int_of(v, x));
        match tag.as_str() {
            "prim" => {
                let name = xs.get(1).map(|&x| str_of(v, x)).unwrap_or_default();
                prim(&name).map_or(Mean::Undefined, |(n, c, h)| Mean::Prim(n, c, h))
            }
            "macro" => Mean::Macro(Rc::new(Macro::new(
                u8::try_from(n(1)).unwrap_or(0),
                toks_of(v, xs.get(2).copied()),
                toks_rc(v, xs.get(3).copied()),
            ))),
            "char" => Mean::Char(
                u8::try_from(n(1)).unwrap_or(0),
                u32::try_from(n(2)).unwrap_or(0),
            ),
            "font" => Mean::Font(n(1)),
            "chardef" => Mean::CharDef(n(1)),
            "mathchardef" => Mean::MathCharDef(n(1)),
            "countdef" => Mean::Reg(
                cmd::ASSIGN_INT,
                COUNT_BASE + i32::try_from(n(1)).unwrap_or(0),
            ),
            "dimendef" => Mean::Reg(
                cmd::ASSIGN_DIMEN,
                SCALED_BASE + i32::try_from(n(1)).unwrap_or(0),
            ),
            "skipdef" => Mean::Reg(
                cmd::ASSIGN_GLUE,
                SKIP_BASE + i32::try_from(n(1)).unwrap_or(0),
            ),
            "muskipdef" => Mean::Reg(
                cmd::ASSIGN_MU_GLUE,
                MU_SKIP_BASE + i32::try_from(n(1)).unwrap_or(0),
            ),
            "toksdef" => Mean::Reg(
                cmd::ASSIGN_TOKS,
                TOKS_BASE + i32::try_from(n(1)).unwrap_or(0),
            ),
            "notexpanded" => Mean::NotExpanded,
            _ => Mean::Undefined,
        }
    }
}

/// The value of macro `m`, encoded once an epoch per macro (cached by
/// address), and its decoding kept so reading it back decodes nothing.
fn macro_id(v: &Values, m: &Rc<Macro>) -> ValueId {
    let Some(epoch) = scope_epoch(v) else {
        return Mean::Macro(m.clone()).encode_new(v);
    };
    let key = Rc::as_ptr(m) as usize;
    let hit = ENCODED.with(|c| {
        let mut c = c.borrow_mut();
        if c.instance != v.epoch() || c.epoch != epoch {
            crate::prof::caches::hit(crate::prof::caches::ENC_RESET);
            *c = Encoded {
                instance: v.epoch(),
                epoch,
                ..Encoded::default()
            };
        }
        c.macros.get(&key).map(|e| e.1)
    });
    if let Some(x) = hit {
        crate::prof::caches::hit(crate::prof::caches::ENC_MACRO);
        return x;
    }
    let mm = crate::prof::caches::miss(crate::prof::caches::ENC_MACRO);
    let x = Mean::Macro(m.clone()).encode_new(v);
    crate::prof::caches::made(mm);
    remember_macro(v, m, x);
    DECODED.with(|c| {
        let mut c = c.borrow_mut();
        if c.instance != v.epoch() {
            *c = Decoded {
                instance: v.epoch(),
                ..Decoded::default()
            };
        }
        if c.means.len() >= DECODED_MAX {
            c.means.clear();
        }
        c.means.entry(x).or_insert_with(|| Mean::Macro(m.clone()));
    });
    x
}

/// Records `id` as the value of macro `m` at the run's epoch.
fn remember_macro(v: &Values, m: &Rc<Macro>, id: ValueId) {
    let Some(epoch) = scope_epoch(v) else { return };
    ENCODED.with(|c| {
        let mut c = c.borrow_mut();
        if c.instance != v.epoch() || c.epoch != epoch {
            crate::prof::caches::hit(crate::prof::caches::ENC_RESET);
            *c = Encoded {
                instance: v.epoch(),
                epoch,
                ..Encoded::default()
            };
        }
        if c.macros.len() >= DECODED_MAX {
            c.macros.clear();
        }
        c.macros.insert(Rc::as_ptr(m) as usize, (m.clone(), id));
    });
}

fn reg_tag(c: u8) -> (&'static str, i32) {
    match c {
        cmd::ASSIGN_INT => ("countdef", COUNT_BASE),
        cmd::ASSIGN_DIMEN => ("dimendef", SCALED_BASE),
        cmd::ASSIGN_GLUE => ("skipdef", SKIP_BASE),
        cmd::ASSIGN_MU_GLUE => ("muskipdef", MU_SKIP_BASE),
        _ => ("toksdef", TOKS_BASE),
    }
}

/// The field of the eqtb quantity at `(cmd, chr)` for the assignment
/// commands (§1224, §1228): parameters by name, registers by number.
pub(crate) fn quantity_field(c: u8, chr: i32) -> String {
    match quantity_register(c, chr) {
        Some((reg, n)) => format!("{reg}:{n}"),
        None => format!("param:{}", prim_name(c, chr).unwrap_or("?")),
    }
}

/// The registers' field prefixes (`count`, `dimen`, `skip`, `muskip`,
/// `toks`), one string each: [`crate::state::State::numbered`] knows a
/// prefix by its address.
pub(crate) static REG_NAMES: [&str; 5] = ["count", "dimen", "skip", "muskip", "toks"];

/// The code tables' field prefixes (`lc`, `uc`, `sf`, `mc`, `dc`), as
/// [`REG_NAMES`].
pub(crate) static CODE_NAMES: [&str; 5] = ["lc", "uc", "sf", "mc", "dc"];

/// The register a quantity at `(cmd, chr)` is (its family's prefix and
/// number), none for a parameter.
pub(crate) fn quantity_register(c: u8, chr: i32) -> Option<(&'static str, i64)> {
    let (base, reg) = match c {
        cmd::ASSIGN_INT => (COUNT_BASE, 0),
        cmd::ASSIGN_DIMEN => (SCALED_BASE, 1),
        cmd::ASSIGN_GLUE => (SKIP_BASE, 2),
        cmd::ASSIGN_MU_GLUE => (MU_SKIP_BASE, 3),
        _ => (TOKS_BASE, 4),
    };
    (chr >= base).then(|| (REG_NAMES[reg], i64::from(chr - base)))
}

/// A glue value: `[width, stretch, stretch order, shrink, shrink order]`.
pub(crate) fn glue_value(v: &Values, g: &Glue) -> ValueId {
    encoded(
        v,
        |c| &mut c.glues,
        *g,
        || {
            v.list(vec![
                int_id(v, i64::from(g.width)),
                int_id(v, i64::from(g.stretch)),
                int_id(v, g.stretch_order as i64),
                int_id(v, i64::from(g.shrink)),
                int_id(v, g.shrink_order as i64),
            ])
        },
    )
}

pub(crate) fn order(i: i64) -> Order {
    match i {
        1 => Order::Fil,
        2 => Order::Fill,
        3 => Order::Filll,
        _ => Order::Normal,
    }
}

pub(crate) fn glue_of(v: &Values, id: Option<ValueId>) -> Glue {
    let Some(id) = id else { return Glue::ZERO };
    decoded(v, id, |d| &mut d.glues, || glue_new(v, id))
}

fn glue_new(v: &Values, id: ValueId) -> Glue {
    v.with(id, |x| glue_from(v, x))
}

fn glue_from(v: &Values, x: &Value) -> Glue {
    match x {
        Value::List(xs) if xs.len() == 5 => {
            let n = |k: usize| i32::try_from(int_of(v, xs[k])).unwrap_or(0);
            Glue {
                width: n(0),
                stretch: n(1),
                stretch_order: order(int_of(v, xs[2])),
                shrink: n(3),
                shrink_order: order(int_of(v, xs[4])),
            }
        }
        _ => Glue::ZERO,
    }
}

pub(crate) fn i32_of(v: &Values, id: Option<ValueId>) -> i32 {
    id.and_then(|x| v.as_int(x)).map_or(0, |i| {
        i32::try_from(i).unwrap_or(if i < 0 { i32::MIN } else { i32::MAX })
    })
}

/// INITEX's state (§222–§240) that differs from zero, void, empty and
/// undefined: the category codes, the other code tables, the parameters,
/// and the frozen control sequences.
pub(crate) fn initex(v: &Values, xetex: bool) -> Vec<(String, ValueId)> {
    let mut out = Vec::new();
    for k in 0..256i64 {
        let c = u8::try_from(k).unwrap_or(0);
        // §232
        let cat = match c {
            b'\\' => 0,
            b'%' => 14,
            b'a'..=b'z' | b'A'..=b'Z' => 11,
            127 => 15,
            0 => 9,
            13 => 5,
            b' ' => 10,
            _ => 12,
        };
        out.push((format!("cat:{k}"), v.int(cat)));
        let (lc, uc, sf) = match c {
            b'a'..=b'z' => (k, k - 32, 1000),
            b'A'..=b'Z' => (k + 32, k, 999),
            _ => (0, 0, 1000),
        };
        if lc != 0 {
            out.push((format!("lc:{k}"), v.int(lc)));
            out.push((format!("uc:{k}"), v.int(uc)));
        }
        out.push((format!("sf:{k}"), v.int(sf)));
        // (XeTeX's math codes: class from bit 21, family from bit 24,
        // `set_class_field`, `set_family_field`)
        let mc = match (c, xetex) {
            (b'0'..=b'9', false) => k + 0x7000,
            (b'a'..=b'z' | b'A'..=b'Z', false) => k + 0x7100,
            (b'0'..=b'9', true) => k + (7 << 21),
            (b'a'..=b'z' | b'A'..=b'Z', true) => k + (7 << 21) + (1 << 24),
            _ => k,
        };
        out.push((format!("mc:{k}"), v.int(mc)));
        out.push((format!("dc:{k}"), v.int(if c == b'.' { 0 } else { -1 })));
    }
    for (p, x) in [
        ("tolerance", 10000),
        ("mag", 1000),
        ("maxdeadcycles", 25),
        ("escapechar", 92),
        ("endlinechar", 13),
        ("hangafter", 1),
    ] {
        out.push((format!("param:{p}"), v.int(x)));
    }
    let relax = Mean::Prim("relax", cmd::RELAX, 256).encode(v);
    out.push(("cs:\u{10FFFE}relax".to_owned(), relax));
    // (`frozen_fi`, §222: what §336 inserts to end a conditional skipped
    // into an outer macro)
    let fi = Mean::Prim("fi", cmd::FI_OR_ELSE, crate::expand::FI_CODE).encode(v);
    out.push(("cs:\u{10FFFE}fi".to_owned(), fi));
    out.push((
        "cs:\u{10FFFE}endgraf".to_owned(),
        v.list(vec![v.str("prim"), v.str("\u{10FFFE}endgraf")]),
    ));
    for n in [
        "\u{10FFFE}alignpeek",
        "\u{10FFFE}cr",
        "\u{10FFFE}endgroup",
        "\u{10FFFE}right",
    ] {
        out.push((format!("cs:{n}"), v.list(vec![v.str("prim"), v.str(n)])));
    }
    if xetex {
        // (xetex.web: "for backward compatibility with standard TeX")
        out.push(("param:XeTeXhyphenatablelength".to_owned(), v.int(63)));
        for (n, _, _) in crate::xetex::initex_prims() {
            out.push((format!("cs:{n}"), v.list(vec![v.str("prim"), v.str(n)])));
        }
    } else {
        out.extend(crate::pdfprim::initex(v));
        for &(n, _, _) in crate::etex::all_prims() {
            out.push((format!("cs:{n}"), v.list(vec![v.str("prim"), v.str(n)])));
        }
    }
    // §552: the null font's name
    out.push((
        "fontinfo:0".to_owned(),
        v.list(vec![v.str("nullfont"), v.int(0), v.int(0)]),
    ));
    out
}

/// The tag of [`CatMonoid`]'s pairs.
const CAT: &str = "\u{1}cat";

/// A fold whose combination is a pair (`[CAT, a, b]`), not a sequence's
/// join: an accumulator the link alone reads whole, which an edit folds
/// again over every segment (a rope's join each, ~3 µs: a whole line's
/// fold of some thousand segments, ms; a pair, a value made). Its
/// contributions are the elements themselves (no sequence of one), read
/// with [`cat_to_vec`] or walked with a [`CatMemo`].
pub(crate) struct CatMonoid;

impl phitex_defuse::Monoid for CatMonoid {
    fn empty(&self, _: &Values) -> ValueId {
        ValueId::NIL
    }

    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        if a == ValueId::NIL {
            b
        } else if b == ValueId::NIL {
            a
        } else {
            v.list(vec![v.str(CAT), a, b])
        }
    }
}

/// A [`CatMonoid`] pair's two sides (`tag`: `v.str(CAT)`).
fn cat_pair(v: &Values, tag: ValueId, x: ValueId) -> Option<(ValueId, ValueId)> {
    v.with_list(x, |xs| match xs {
        [t, a, b] if *t == tag => Some((*a, *b)),
        _ => None,
    })
    .flatten()
}

/// A [`CatMonoid`] fold's elements in order.
pub(crate) fn cat_to_vec(v: &Values, x: ValueId) -> Vec<ValueId> {
    let tag = v.str(CAT);
    let mut out = Vec::new();
    let mut stack = vec![x];
    while let Some(x) = stack.pop() {
        if x == ValueId::NIL {
            continue;
        }
        match cat_pair(v, tag, x) {
            Some((a, b)) => {
                stack.push(b);
                stack.push(a);
            }
            None => out.push(x),
        }
    }
    out
}

/// A [`CatMonoid`] fold's elements mapped by a function, from the last
/// walk's: a fold is the pairs of its segments' sums, each pair on the
/// last one (`[[[s1, s2], s3], s4]`), so an edit's fold shares the pairs
/// below the first segment it changed. The walk goes down from the top
/// to a pair the last walk met (its elements before it are the last
/// walk's first ones), mapping the sums above it, each kept by its value.
pub(crate) struct CatMemo<T> {
    inst: u64,
    /// The pairs down the left side of the last walk's fold, each with
    /// the count of elements under it.
    spine: FxHashMap<ValueId, usize>,
    /// The last walk's elements.
    out: Vec<T>,
    /// A segment's sum mapped, by value.
    sums: FxHashMap<ValueId, std::sync::Arc<[T]>>,
    elems: FxHashMap<ValueId, Option<T>>,
}

impl<T> Default for CatMemo<T> {
    fn default() -> Self {
        CatMemo {
            inst: 0,
            spine: FxHashMap::default(),
            out: Vec::new(),
            sums: FxHashMap::default(),
            elems: FxHashMap::default(),
        }
    }
}

impl<T: Clone> CatMemo<T> {
    /// The elements of fold `x` mapped by `f` (`None`: left out), in
    /// order; `f` must be a function of the element alone.
    pub(crate) fn walk(
        &mut self,
        v: &Values,
        x: ValueId,
        f: impl Fn(ValueId) -> Option<T>,
    ) -> Vec<T> {
        if self.inst != v.epoch() {
            *self = CatMemo {
                inst: v.epoch(),
                ..CatMemo::default()
            };
        }
        let tag = v.str(CAT);
        // (down the left side to a pair the last walk met, or the end)
        let mut above: Vec<(ValueId, ValueId)> = Vec::new();
        let mut cur = x;
        let base = loop {
            if cur == ValueId::NIL {
                break 0;
            }
            if let Some(&n) = self.spine.get(&cur) {
                break n;
            }
            if let Some((a, b)) = cat_pair(v, tag, cur) {
                above.push((cur, b));
                cur = a;
            } else {
                above.push((cur, cur));
                break 0;
            }
        };
        self.out.truncate(base);
        self.spine.retain(|_, n| *n <= base);
        let mut met = 0usize;
        for &(node, sum) in above.iter().rev() {
            let m = if let Some(m) = self.sums.get(&sum) {
                m.clone()
            } else {
                let mut m = Vec::new();
                for e in cat_to_vec(v, sum) {
                    met += 1;
                    let t = if let Some(t) = self.elems.get(&e) {
                        t.clone()
                    } else {
                        let t = f(e);
                        self.elems.insert(e, t.clone());
                        t
                    };
                    m.extend(t);
                }
                let m: std::sync::Arc<[T]> = m.into();
                self.sums.insert(sum, m.clone());
                m
            };
            self.out.extend_from_slice(&m);
            self.spine.insert(node, self.out.len());
        }
        // (the kept sums and elements dropped once grown past four times
        // the fold's: mapped again where met)
        if self.sums.len() > 4 * self.spine.len() + 1024 {
            self.sums.clear();
        }
        if self.elems.len() > 4 * (met + self.out.len()) + 1024 {
            self.elems.clear();
        }
        self.out.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phitex_defuse::Monoid as _;

    /// A fold as the engine makes it: each segment's sum, then the sums
    /// left to right.
    fn fold(v: &Values, segs: &[Vec<i64>]) -> ValueId {
        let m = CatMonoid;
        let mut acc = m.empty(v);
        for s in segs {
            let mut sum = m.empty(v);
            for &x in s {
                sum = m.combine(v, sum, v.int(x));
            }
            acc = m.combine(v, acc, sum);
        }
        acc
    }

    /// A walk from the last walk's spine gives the fold's elements,
    /// however the segments changed (one edited, added, removed, emptied)
    /// and whichever the last walk was.
    #[test]
    fn cat_walks_are_the_folds_elements() {
        let v = Values::new();
        let mut segs: Vec<Vec<i64>> = (0..200)
            .map(|k| (0..(k % 5)).map(|j| k * 10 + j).collect())
            .collect();
        let mut memo: CatMemo<i64> = CatMemo::default();
        let step = |segs: &Vec<Vec<i64>>, memo: &mut CatMemo<i64>| {
            let x = fold(&v, segs);
            let want: Vec<i64> = segs.iter().flatten().copied().collect();
            let got = memo.walk(&v, x, |e| v.as_int(e));
            assert_eq!(got, want);
            assert_eq!(cat_to_vec(&v, x).len(), want.len());
        };
        step(&segs, &mut memo);
        for k in [150usize, 3, 199, 0, 77] {
            segs[k].push(-1);
            step(&segs, &mut memo);
            segs.insert(k, vec![-2, -3]);
            step(&segs, &mut memo);
            segs.remove(k / 2);
            step(&segs, &mut memo);
            segs[k / 3].clear();
            step(&segs, &mut memo);
        }
    }
}
