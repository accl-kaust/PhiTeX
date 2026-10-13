//! Values: tokens, meanings, glue and the codes TeX gives them.
//!
//! A token is `Int(cat << 21 | char)` for a character (an active character
//! has category 13) or `Str("\\name")` for a control sequence (§289).
//! A meaning is a `List` tagged by its first element (§222–§225).

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
    /// Whether the name is a frozen or internal one (`\u{10FFFE}…`).
    frozen: bool,
    name: Box<str>,
}

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
            static LOCAL: std::cell::RefCell<FxHashMap<Box<str>, CsId>> =
                std::cell::RefCell::default();
        }
        static TABLE: OnceLock<std::sync::Mutex<FxHashMap<&'static str, CsId>>> = OnceLock::new();
        if let Some(c) = LOCAL.with(|l| l.borrow().get(name).copied()) {
            return c;
        }
        let c = {
            let mut t = TABLE
                .get_or_init(Default::default)
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(&c) = t.get(name) {
                c
            } else {
                let id = t.len();
                let e: &'static CsEntry = Box::leak(Box::new(CsEntry {
                    id: u32::try_from(id).unwrap_or(u32::MAX),
                    frozen: name.starts_with('\u{10FFFE}'),
                    name: Box::from(name),
                }));
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
                t.insert(&e.name, c);
                c
            }
        };
        LOCAL.with(|l| l.borrow_mut().insert(Box::from(name), c));
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

    /// Whether the name is a frozen or internal one (`\u{10FFFE}…`).
    #[inline]
    pub(crate) fn frozen(self) -> bool {
        self.0.frozen
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
/// The bit of a frozen or internal control sequence token (its name
/// starts with `\u{10FFFE}`): tested without touching the entry.
const FROZEN_BIT: u32 = 1 << 30;
const CS_MASK: u32 = CS_BIT | FROZEN_BIT;

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
        let f = if c.frozen() { FROZEN_BIT } else { 0 };
        Tok(CS_BIT | f | u32::try_from(c.index()).unwrap_or(0))
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
        (self.0 & CS_BIT != 0).then(|| CsId::from_index((self.0 & !CS_MASK) as usize))
    }

    /// The number of a control sequence token ([`CsId::index`]).
    #[inline]
    pub(crate) fn cs_index(self) -> Option<usize> {
        (self.0 & CS_BIT != 0).then_some((self.0 & !CS_MASK) as usize)
    }

    #[inline]
    pub(crate) fn is_cs(self) -> bool {
        self.0 & CS_BIT != 0
    }

    /// Whether this is a frozen or internal control sequence (one bit,
    /// no entry lookup).
    #[inline]
    pub(crate) fn is_frozen_cs(self) -> bool {
        self.0 & CS_MASK == CS_MASK
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
            Some(n) => Some(format!("cs:{n}")),
            None if self.cat() == Some(13) => Some(format!("act:{}", self.chr())),
            None => None,
        }
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub(crate) fn encode(&self, v: &Values) -> ValueId {
        match self.as_cs() {
            None => v.int(i64::from(self.0)),
            Some(n) => v.str(&format!("\\{n}")),
        }
    }

    pub(crate) fn decode(v: &Values, id: ValueId) -> Tok {
        match &*v.get(id) {
            Value::Int(i) => Tok::c(u32::try_from(*i).unwrap_or(0)),
            Value::Str(s) => Tok::cs(s.strip_prefix('\\').unwrap_or(s)),
            _ => Tok::c(0),
        }
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
            || match &*v.get(id) {
                Value::List(xs) => xs.iter().map(|&x| Tok::decode(v, x)).collect(),
                _ => Rc::from([]),
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
        if c.instance != v.instance() || c.epoch != epoch {
            *c = Encoded {
                instance: v.instance(),
                epoch,
                ..Encoded::default()
            };
        }
        map(&mut c).get(&key).copied()
    });
    if let Some(x) = hit {
        return x;
    }
    let x = make();
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
    encoded(v, |c| &mut c.ints, i, || v.int(i))
}

/// The value of string `s` (cached in a run).
pub(crate) fn str_id(v: &Values, s: &str) -> ValueId {
    if let Some(e) = scope_epoch(v) {
        let hit = ENCODED.with(|c| {
            let c = c.borrow();
            (c.instance == v.instance() && c.epoch == e)
                .then(|| c.strs.get(s).copied())
                .flatten()
        });
        if let Some(x) = hit {
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
        if c.instance != v.instance() || c.epoch != epoch {
            *c = Encoded {
                instance: v.instance(),
                epoch,
                ..Encoded::default()
            };
        }
        c.toks.get(&t).copied()
    });
    if let Some(x) = hit {
        return x;
    }
    let x = t.encode(v);
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
    DECODED.with(|c| {
        let c = c.borrow();
        (c.instance == v.instance())
            .then(|| c.leaves.get(&id).cloned())
            .flatten()
    })
}

/// Keeps leaf node `n`, the decoding of `id` ([`leaf_of`]).
pub(crate) fn keep_leaf(v: &Values, id: ValueId, n: &phitex_typeset::node::Node) {
    DECODED.with(|c| {
        let mut c = c.borrow_mut();
        if c.instance != v.instance() {
            *c = Decoded {
                instance: v.instance(),
                ..Decoded::default()
            };
        }
        if c.leaves.len() >= DECODED_MAX {
            c.leaves.clear();
        }
        c.leaves.insert(id, n.clone());
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
        if c.instance != v.instance() {
            *c = Decoded {
                instance: v.instance(),
                ..Decoded::default()
            };
        }
        map(&mut c).get(&id).cloned()
    });
    if let Some(x) = hit {
        return x;
    }
    let x = make();
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
    match &*v.get(id) {
        Value::List(xs) => xs.to_vec(),
        Value::Seq(_) | Value::Nil => v.seq_to_vec(id),
        _ => Vec::new(),
    }
}

pub(crate) fn int_of(v: &Values, id: ValueId) -> i64 {
    v.as_int(id).unwrap_or(0)
}

pub(crate) fn int_at(v: &Values, id: ValueId, k: usize) -> i64 {
    match &*v.get(id) {
        Value::List(xs) => xs.get(k).map_or(0, |&x| int_of(v, x)),
        _ => 0,
    }
}

pub(crate) fn str_of(v: &Values, id: ValueId) -> String {
    match &*v.get(id) {
        Value::Str(s) => s.to_string(),
        _ => String::new(),
    }
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
        crate::etex::all_prims()
            .map(|&(n, c, h)| (n, (c, h)))
            .collect()
    })
}

fn prim_names() -> &'static phitex_defuse::FxHashMap<(u8, i32), &'static str> {
    static M: OnceLock<phitex_defuse::FxHashMap<(u8, i32), &'static str>> = OnceLock::new();
    M.get_or_init(|| {
        crate::etex::all_prims()
            .map(|&(n, c, h)| ((c, h), n))
            .collect()
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
    prim_index().get(base).map(|&(c, h)| {
        let n = crate::etex::all_prims()
            .find(|p| p.0 == base)
            .map_or("", |p| p.0);
        (n, c, h)
    })
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
        let xs = match &*v.get(id) {
            Value::List(xs) => xs.clone(),
            _ => return Mean::Undefined,
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
            "macro" => Mean::Macro(Rc::new(Macro {
                flags: u8::try_from(n(1)).unwrap_or(0),
                params: toks_of(v, xs.get(2).copied()),
                body: toks_rc(v, xs.get(3).copied()),
            })),
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
        if c.instance != v.instance() || c.epoch != epoch {
            *c = Encoded {
                instance: v.instance(),
                epoch,
                ..Encoded::default()
            };
        }
        c.macros.get(&key).map(|e| e.1)
    });
    if let Some(x) = hit {
        return x;
    }
    let x = Mean::Macro(m.clone()).encode_new(v);
    remember_macro(v, m, x);
    DECODED.with(|c| {
        let mut c = c.borrow_mut();
        if c.instance != v.instance() {
            *c = Decoded {
                instance: v.instance(),
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
        if c.instance != v.instance() || c.epoch != epoch {
            *c = Encoded {
                instance: v.instance(),
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
    let (base, reg) = match c {
        cmd::ASSIGN_INT => (COUNT_BASE, "count"),
        cmd::ASSIGN_DIMEN => (SCALED_BASE, "dimen"),
        cmd::ASSIGN_GLUE => (SKIP_BASE, "skip"),
        cmd::ASSIGN_MU_GLUE => (MU_SKIP_BASE, "muskip"),
        _ => (TOKS_BASE, "toks"),
    };
    if chr >= base {
        format!("{reg}:{}", chr - base)
    } else {
        format!("param:{}", prim_name(c, chr).unwrap_or("?"))
    }
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
    match &*v.get(id) {
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
pub(crate) fn initex(v: &Values) -> Vec<(String, ValueId)> {
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
        let mc = match c {
            b'0'..=b'9' => k + 0x7000,
            b'a'..=b'z' | b'A'..=b'Z' => k + 0x7100,
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
    out.extend(crate::pdfprim::initex(v));
    for &(n, _, _) in crate::etex::all_prims() {
        out.push((format!("cs:{n}"), v.list(vec![v.str("prim"), v.str(n)])));
    }
    // §552: the null font's name
    out.push((
        "fontinfo:0".to_owned(),
        v.list(vec![v.str("nullfont"), v.int(0), v.int(0)]),
    ));
    out
}
