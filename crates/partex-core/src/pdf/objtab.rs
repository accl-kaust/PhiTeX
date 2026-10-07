//! pdfTeX part 32d: the cross-reference table (pdfTeX §695–§699) and
//! avlstuff.c's per-type lookup trees.
//!
//! Object numbers are observable (`\pdflastobj`, and every reference in
//! the PDF file), so objects are created in exactly pdfTeX's order.
//! What pdfTeX keeps in `pdf_mem` behind `obj_aux` is typed here.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::val::{Element, VMap, VTab};

use partex_engine::node::Tokens;

/// The types of objects (pdfTeX §695): the lists `head_tab` heads.
pub(crate) const OBJ_TYPE_OTHERS: usize = 0;
pub(crate) const OBJ_TYPE_PAGE: usize = 1;
pub(crate) const OBJ_TYPE_PAGES: usize = 2;
pub(crate) const OBJ_TYPE_FONT: usize = 3;
pub(crate) const OBJ_TYPE_OUTLINE: usize = 4;
pub(crate) const OBJ_TYPE_DEST: usize = 5;
pub(crate) const OBJ_TYPE_STRUCT_DEST: usize = 6;
pub(crate) const OBJ_TYPE_OBJ: usize = 7;
pub(crate) const OBJ_TYPE_XFORM: usize = 8;
pub(crate) const OBJ_TYPE_XIMAGE: usize = 9;
pub(crate) const OBJ_TYPE_THREAD: usize = 10;
pub(crate) const HEAD_TAB_MAX: usize = OBJ_TYPE_THREAD;

/// Whether a virtual object that TeX itself identifies is named by that
/// identity instead of by the position of the step that made it (the
/// host's switch, `PARTEX_MACHINE_TREE_NAMES=0` turns it off; DESIGN.md
/// §7.16.5).
pub static TREE_NAMES: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// What names a virtual object that TeX identifies, if it is one: its
/// type and identifier where these are unique in a run (a page by its
/// number, a destination by its name, a raw object, form or image by
/// its count, a font by its number). A leaf of the page tree has no
/// identifier of its own: its caller names it (`create_named`).
fn tex_identity(t: usize, i: &Id) -> Option<u128> {
    let unique = matches!(
        t,
        OBJ_TYPE_PAGE
            | OBJ_TYPE_FONT
            | OBJ_TYPE_DEST
            | OBJ_TYPE_STRUCT_DEST
            | OBJ_TYPE_OBJ
            | OBJ_TYPE_XFORM
            | OBJ_TYPE_XIMAGE
            | OBJ_TYPE_THREAD
    );
    (unique && TREE_NAMES.load(core::sync::atomic::Ordering::Relaxed))
        .then(|| partex_engine::stablehash::StableHasher::of(&(b"tex", t, i)))
}

/// Whether a machine's region that asks the numbering for single numbers
/// (`final_num`, `of_final`) guards the answers instead of the whole
/// numbering (the host's switch, `PARTEX_MACHINE_NUM_ANSWERS=0` turns it
/// off; DESIGN.md §7.16.5).
pub static NUM_ANSWERS: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// An answer the numbering gave (`ObjLog::answers`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NumAnswer {
    /// `final_num(k)` was `n`.
    Final(i32, i32),
    /// `of_final(n)` was this object.
    Of(i32, Option<i32>),
}

/// An object's identifier (`obj_info`): a number, or a name (pdfTeX
/// stores a string number, negated).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Id {
    Num(i32),
    Name(Arc<[u8]>),
}

partex_engine::persist_enum!(Id { Num(a0), Name(a0) });

impl Id {
    /// As bytes (a machine's `MCell::Tree` names an entry by them).
    pub(crate) fn key(&self) -> Arc<[u8]> {
        match self {
            Self::Num(n) => [&[0u8][..], &n.to_le_bytes()].concat().into(),
            Self::Name(s) => [&[1u8][..], s].concat().into(),
        }
    }

    /// [`Id::key`]'s identifier.
    pub(crate) fn of_key(k: &[u8]) -> Self {
        match k.split_first() {
            Some((0, n)) => Self::Num(i32::from_le_bytes(n.try_into().unwrap_or([0; 4]))),
            Some((_, s)) => Self::Name(s.into()),
            None => Self::Num(0),
        }
    }

    pub(crate) fn num(&self) -> i32 {
        match self {
            Self::Num(n) => *n,
            Self::Name(_) => 0,
        }
    }
}

/// avlstuff.c's `compare_info`: names by length, then bytes; numbers as
/// numbers (the two never meet in one tree).
impl Ord for Id {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        match (self, other) {
            (Self::Name(a), Self::Name(b)) => a.len().cmp(&b.len()).then_with(|| a.cmp(b)),
            (Self::Num(a), Self::Num(b)) => a.cmp(b),
            (Self::Name(_), Self::Num(_)) => core::cmp::Ordering::Less,
            (Self::Num(_), Self::Name(_)) => core::cmp::Ordering::Greater,
        }
    }
}

impl PartialOrd for Id {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// A raw object (`\pdfobj`; `pdfmem_obj_size`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RawObj {
    pub data: Tokens,
    pub is_stream: bool,
    pub stream_attr: Option<Tokens>,
    pub is_file: bool,
}

partex_engine::persist_struct!(RawObj {
    data,
    is_stream,
    stream_attr,
    is_file
});

/// A form `XObject` (`\pdfxform`; `pdfmem_xform_size`). The box is kept
/// until the form is written.
#[derive(Clone, Debug, PartialEq, Hash)]
pub(crate) struct XForm {
    pub width: i32,
    pub height: i32,
    pub depth: i32,
    pub boxed: Option<partex_engine::node::BoxNode>,
    pub attr: Option<Tokens>,
    pub resources: Option<Tokens>,
}

partex_engine::persist_struct!(XForm {
    width,
    height,
    depth,
    boxed,
    attr,
    resources
});

/// An image `XObject` (`\\pdfximage`; `pdfmem_ximage_size`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct XImage {
    pub width: i32,
    pub height: i32,
    pub depth: i32,
    pub attr: Option<Tokens>,
    pub image: Option<alloc::sync::Arc<super::image::Image>>,
    /// `img_group_ref`: a PDF page's `/Group` object (0: none, -1: not
    /// numbered yet).
    pub group_ref: i32,
}

partex_engine::persist_struct!(XImage {
    width,
    height,
    depth,
    attr,
    image,
    group_ref
});

/// An outline entry (`pdfmem_outline_size`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Outline {
    pub title: i32,
    pub parent: i32,
    pub prev: i32,
    pub next: i32,
    pub first: i32,
    pub last: i32,
    pub action_objnum: i32,
    pub count: i32,
    pub attr: Option<Tokens>,
}

partex_engine::persist_struct!(Outline {
    title,
    parent,
    prev,
    next,
    first,
    last,
    action_objnum,
    count,
    attr
});

/// A bead of an article thread (`pdfmem_bead_size`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Bead {
    /// `obj_bead_rect`: the rectangle's object once written.
    pub rect: i32,
    pub page: i32,
    pub next: i32,
    pub prev: i32,
    pub attr: Option<Tokens>,
}

partex_engine::persist_struct!(Bead {
    rect,
    page,
    next,
    prev,
    attr
});

/// What `obj_aux` points to.
#[derive(Clone, Debug, Default, PartialEq, Hash)]
pub(crate) enum Aux {
    #[default]
    None,
    /// A plain number (`obj_aux` used as a counter or a pointer whose
    /// only use is being nonzero).
    Int(i32),
    Obj(Box<RawObj>),
    XForm(Box<XForm>),
    XImage(Box<XImage>),
    Outline(Box<Outline>),
    Bead(Box<Bead>),
    /// An annotation or link on the page being shipped
    /// (`obj_annot_ptr`).
    Mark(Box<super::ship::Mark>),
    /// A destination that has been placed (`obj_dest_ptr`).
    Dest(Box<super::ship::Dest>),
}

partex_engine::persist_enum!(Aux { None, Int(a0), Obj(a0), XForm(a0), XImage(a0), Outline(a0), Bead(a0), Mark(a0), Dest(a0) });

/// One `obj_entry`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    pub info: Id,
    pub link: i32,
    /// Negative: -2 fresh, -1 scheduled; else the byte offset (or, in an
    /// object stream, the stream's object number).
    pub offset: i64,
    pub os_idx: i32,
    pub aux: Aux,
}

partex_engine::persist_struct!(Entry {
    info,
    link,
    offset,
    os_idx,
    aux
});

/// The state hash leaves out a written object's byte offset (see
/// `PdfOut`'s hash); an object stream's number stays in.
impl core::hash::Hash for Entry {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        let Self {
            info,
            link,
            offset,
            os_idx,
            aux,
        } = self;
        info.hash(h);
        link.hash(h);
        if *os_idx == -1 && *offset >= 0 {
            (-3i64).hash(h); // (written, somewhere)
        } else {
            offset.hash(h);
        }
        os_idx.hash(h);
        aux.hash(h);
    }
}

/// Every field but a written object's byte offset: the build links its
/// files, and the link places every object (DESIGN 7.17.3), so where an
/// object is in the file is not state; an object stream's number stays.
impl Element for Entry {
    fn element_version(&self) -> u128 {
        let Self {
            info,
            link,
            offset,
            os_idx,
            aux,
        } = self;
        let at = if *os_idx == -1 && *offset >= 0 {
            -3 // (written, somewhere)
        } else {
            *offset
        };
        partex_ssa::Version::of(&(info, link, at, os_idx, aux)).0
    }
}

impl Entry {
    /// A byte offset in the file (not an object stream's number).
    pub(crate) fn at_byte(&self) -> bool {
        self.os_idx == -1 && self.offset >= 0
    }
}

/// `obj_tab`, `head_tab` and the lookup trees.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ObjTab {
    /// Index 0 unused, as object 0 is not an object.
    /// (a table of shared chunks, each entry with its version: the
    /// `OBJS` field's value)
    pub tab: VTab<Entry>,
    pub head: [i32; HEAD_TAB_MAX + 1],
    /// `obj_ptr`: the last object users can see.
    pub obj_ptr: i32,
    /// avlstuff.c's `PdfObjTree`: the first object of each type and
    /// identifier.
    /// (persistent maps carrying their versions: the `OBJ_TREES` field)
    pub(crate) trees: [VMap<Id, i32>; HEAD_TAB_MAX + 1],
    /// `dest_names`: named destinations in order of creation.
    /// (the `DESTS` field)
    pub dest_names: VTab<(Arc<[u8]>, i32)>,
    /// A running hash of `dest_names` (a machine's `MCell::Dests`
    /// version).
    pub(crate) dest_hash: u128,
    /// Symbolic object streams (`PdfOut::symbolic`): where a written
    /// object went is the link's, not state.
    pub symbolic: bool,
    /// A machine's cells (`ObjLog`).
    pub(crate) log: ObjLog,
    /// Virtual object numbers (`vnum.rs`, machine mode): objects are
    /// `vtab`'s, by virtual id (`tab` keeps object 0 only), and what
    /// pdfTeX's numbers depend on is `alog`'s.
    pub(crate) virt: bool,
    /// (in shared shards: a snapshot copies the shards that changed)
    pub(crate) vtab: crate::cow::ShardMap<Entry>,
    pub(crate) alog: super::vnum::Log,
    /// Where the step making objects is (a hash of its position) and
    /// how many it made so far: the next virtual id's seed.
    pub(crate) vseed: u64,
    pub(crate) vcount: u32,
    /// `alog` replayed (scratch: a cache).
    ncache: NumCache,
    /// SSA mode's virtual numbers (DESIGN 3.12, "PDF object numbers").
    pub(crate) ssa: SsaObjs,
}

partex_engine::persist_struct!(ObjTab {
    tab,
    head,
    obj_ptr,
    trees,
    dest_names,
    dest_hash,
    symbolic,
    log,
    virt,
    vtab,
    alog,
    vseed,
    vcount,
    ncache,
    ssa
});

/// [`ObjTab::numbering`]'s cache: not state (equal to any other, saved
/// as nothing).
#[derive(Clone, Default)]
struct NumCache(Option<Arc<super::vnum::Numbering>>);

impl PartialEq for NumCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl core::fmt::Debug for NumCache {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("NumCache")
    }
}

impl partex_engine::persist::Persist for NumCache {
    fn save(&self, _: &mut partex_engine::persist::Saver) {}
    fn load(_: &mut partex_engine::persist::Loader) -> Option<Self> {
        Some(Self(None))
    }
}

/// SSA mode's virtual object numbers (DESIGN 3.12, "PDF object
/// numbers"). pdfTeX numbers objects in the order they are made, so an
/// object made or dropped early renumbers every later one; with one slot
/// for the table, every later step that made an object read a changed
/// table and made a changed one. Here an object's number is a virtual
/// id that does not depend on what came before it (`vnum.rs`), the
/// link writes pdfTeX's numbers, and each piece of the table is a slot
/// of its own:
/// - each object's entry (`track::Row::PdfObj`), by its id;
/// - each lookup tree's entry (`track::Row::PdfName`), by its type and
///   identifier;
/// - each step's numbering events (`track::Row::PdfNum`), by the step:
///   an append, read only where TeX observes a number (`\pdflastobj`
///   and friends, a number given back, the job's end), which reads the
///   events of every step before it;
/// - the lists' heads, the `OBJS` field.
///
/// An object TeX identifies (a page, a destination, a font: see
/// [`tex_identity`]) is named by that identity, any other by its step
/// and its count in it, so a step run again names its objects as before.
/// (Scratch beside the state: not saved, equal to any other.)
#[derive(Clone, Default)]
pub(crate) struct SsaObjs {
    pub(crate) on: bool,
    /// The open step, whose objects are named by it.
    pub(crate) step: Option<u32>,
    /// The seed of hashed ids (an applied call's name, or a step's id).
    pub(crate) seed: u128,
    /// The open step's numbering events.
    pub(crate) events: Vec<super::vnum::NumEvent>,
    /// Each step's events as its latest run made them, with their
    /// version (`track::Row::PdfNum`'s values).
    pub(crate) steplogs: alloc::collections::BTreeMap<u32, (Arc<[super::vnum::NumEvent]>, u128)>,
    /// The numbering after the steps before the open one (made where the
    /// step first observes a number), and the slots it read.
    pub(crate) prefix: Option<Prefix>,
    /// Who made each hashed id (to tell a collision from the same object
    /// made again).
    vowner: alloc::collections::BTreeMap<i32, u128>,
    /// The lookup trees' entries by their slot.
    pub(crate) name_keys: alloc::collections::BTreeMap<i64, (u8, Id)>,
    /// The entries' versions before their first change since the last
    /// flush (`Tex::obj_flush`).
    pub(crate) before: alloc::collections::BTreeMap<i32, u128>,
    /// The lookup trees' entries read (`false`, with the version read)
    /// and written (`true`) since the last flush, in order.
    pub(crate) names: Vec<(bool, i64, u128)>,
}

impl PartialEq for SsaObjs {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl core::fmt::Debug for SsaObjs {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SsaObjs({})", self.on)
    }
}

impl partex_engine::persist::Persist for SsaObjs {
    fn save(&self, _: &mut partex_engine::persist::Saver) {}
    fn load(_: &mut partex_engine::persist::Loader) -> Option<Self> {
        Some(Self::default())
    }
}

/// The numbering of the steps before the open one, and the slots read
/// for it (each step's events, with their version).
pub(crate) type Prefix = (Arc<super::vnum::Numbering>, Arc<[(u32, u128)]>);

/// The version of an absent object or tree entry.
const ABSENT_CELL: u128 = 0x6162_7365_6e74;

/// The slot of lookup tree `t`'s entry for `i`.
#[must_use]
pub(crate) fn name_slot(t: u8, i: &Id) -> i64 {
    #[allow(clippy::cast_possible_truncation, reason = "a slot's 64 bits")]
    let k = partex_engine::stablehash::StableHasher::of(&(b"pdfname", t, i)) as u64;
    k.cast_signed()
}

/// Structured ids (a step's objects): `1 + (step << SEQ_BITS | count)`,
/// below `HASHED`; ids from `HASHED` on are hashed.
const SEQ_BITS: u32 = 12;
const HASHED: i32 = 1 << 30;

/// A cell of the table named by what it holds: a lookup tree's entry
/// (type and identifier).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum NameCell {
    Tree(u8, Id),
}

/// With a machine (`on`), the table's entries, lookup trees and
/// destination names are cells of their own (`machine.rs`: `MCell::Obj`,
/// `MCell::Name`), not `Rest`'s: two runs that numbered a few objects in
/// another order (a destination made on another page) differ in those
/// cells alone, which few later regions read. What a region did to them
/// is logged here (scratch: cleared at each cut, empty in a clone).
#[derive(Default)]
pub(crate) struct ObjLog {
    pub(crate) on: bool,
    /// The entries read (`k`) and written (`-1 - k`), in order (reads
    /// come through `&self`: an append that finds no room sets
    /// `overflow`, and the region then reads every entry).
    pub(crate) objs: crate::relaxed::Log<i32>,
    pub(crate) overflow: crate::relaxed::Flag,
    /// The name cells read (`false`) and written (`true`), in order.
    pub(crate) names: Vec<(bool, NameCell)>,
    /// The numbering was observed (`ObjTab::numbering`): the region
    /// reads `MCell::Numbering`.
    pub(crate) forced: bool,
    /// The destination names were read (all of them, or how many): the
    /// region reads `MCell::Dests`.
    pub(crate) dests_read: bool,
    /// The answers `final_num` (`Final`) and `of_final` (`Of`) gave since
    /// the last region's end, with `NUM_ANSWERS` on: what a machine's
    /// region guards instead of the whole numbering, unless it also read
    /// it whole (`forced`).
    pub(crate) answers: Vec<NumAnswer>,
    /// The numbering's log length (`alog`) at the last answer: what the
    /// answers depend on of the events the region added.
    pub(crate) answered_at: usize,
    /// Who observed the numbering since the last region's end (a
    /// diagnostic, `PARTEX_MACHINE_PARTS=9`): `final_num` calls, `of_final`
    /// calls, whole readings (`numbers`, the job's end), and the distinct
    /// arguments of the first two.
    pub(crate) observers: (u32, u32, u32, alloc::collections::BTreeSet<i32>),
    /// `objs` sized since the log was made ([`ObjTab::reserve_log`]).
    sized: bool,
    /// The lists' heads (`MCell::PdfWord`, `pdf::word`): bit `t` if the
    /// head of type `t` was read first, bit `16 + t` once written
    /// ([`ObjTab::head`], [`ObjTab::set_head`]).
    heads: core::sync::atomic::AtomicU32,
}

impl Clone for ObjLog {
    fn clone(&self) -> Self {
        Self {
            on: self.on,
            ..Self::default()
        }
    }
}

impl PartialEq for ObjLog {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl core::fmt::Debug for ObjLog {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "ObjLog({})", self.on)
    }
}

impl partex_engine::persist::Persist for ObjLog {
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        self.on.save(s);
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        Some(Self {
            on: bool::load(l)?,
            ..Self::default()
        })
    }
}

/// As derived, but with symbolic object streams a written object is
/// hashed as written, not by where (the link places it).
impl core::hash::Hash for ObjTab {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        let Self {
            tab,
            head,
            obj_ptr,
            trees,
            dest_names,
            dest_hash: _,
            symbolic,
            log: _,
            virt: _,
            vtab: _,
            alog: _,
            vseed: _,
            vcount: _,
            ncache: _,
            ssa: _,
        } = self;
        if self.virt {
            // (virtual numbers: the entries are cells, and how many
            // objects there are is the numbering's; the destination
            // names are `MCell::Dests`)
            (head, symbolic).hash(h);
            return;
        }
        if self.log.on {
            // (the entries, trees and destination names are cells)
            (tab.len(), head, obj_ptr, symbolic).hash(h);
            return;
        }
        if *symbolic {
            tab.len().hash(h);
            for e in tab {
                if e.offset >= 0 {
                    (&e.info, e.link, -3i64, e.os_idx == -1, &e.aux).hash(h);
                } else {
                    e.hash(h);
                }
            }
        } else {
            tab.hash(h);
        }
        (head, obj_ptr, trees, dest_names, symbolic).hash(h);
    }
}

impl Default for ObjTab {
    fn default() -> Self {
        Self {
            tab: VTab::from_vec(alloc::vec![Entry {
                info: Id::Num(0),
                link: 0,
                offset: -2,
                os_idx: 0,
                aux: Aux::None,
            }]),
            head: [0; HEAD_TAB_MAX + 1],
            obj_ptr: 0,
            trees: Default::default(),
            dest_names: VTab::default(),
            dest_hash: 0,
            symbolic: false,
            log: ObjLog::default(),
            virt: false,
            vtab: crate::cow::ShardMap::default(),
            alog: super::vnum::Log::default(),
            vseed: 0,
            vcount: 0,
            ncache: NumCache::default(),
            ssa: SsaObjs::default(),
        }
    }
}

/// `sup_obj_tab_size`.
pub(crate) const SUP_OBJ_TAB_SIZE: usize = 8_388_607;

/// A walk along an object list, from its head to the entry whose link
/// is 0 ([`ObjTab::walk`]), each link read when the walk moves past its
/// entry. It also ends past as many entries as the table holds, which a
/// list made as pdfTeX makes it never reaches. (A step run again has its
/// lists placed whole with their heads, `rebuild::objs_whole`; the bound
/// only keeps a run whose state no run made, which is dropped at its end,
/// DESIGN 7.17.3, from never reaching it.)
pub(crate) struct Walk {
    at: i32,
    started: bool,
    left: usize,
}

impl Walk {
    /// The next entry, if any.
    pub(crate) fn next(&mut self, objs: &ObjTab) -> Option<i32> {
        if self.started && self.at != 0 {
            self.at = objs.get(self.at).link;
        }
        self.started = true;
        if self.at == 0 || self.left == 0 {
            return None;
        }
        self.left -= 1;
        Some(self.at)
    }
}

impl ObjTab {
    /// The head of the list of type `t`, read (logged with a machine).
    pub(crate) fn head(&self, t: usize) -> i32 {
        if self.log.on {
            use core::sync::atomic::Ordering::Relaxed;
            let v = self.log.heads.load(Relaxed);
            if v & (1 << (16 + t)) == 0 {
                self.log.heads.store(v | (1 << t), Relaxed);
            }
        }
        self.head[t]
    }

    /// Set the head of the list of type `t` (logged with a machine).
    pub(crate) fn set_head(&mut self, t: usize, k: i32) {
        if self.log.on {
            *self.log.heads.get_mut() |= 1 << (16 + t);
        }
        self.head[t] = k;
    }

    /// The lists' heads read first and written since the last call (by
    /// bit: type `t` is bit `t`), and none since.
    pub(crate) fn take_heads(&mut self) -> (u16, u16) {
        let v = core::mem::take(self.log.heads.get_mut());
        #[allow(clippy::cast_possible_truncation)] // (the halves)
        (v as u16, (v >> 16) as u16)
    }

    /// The table hashed as [`Hash`] does, without the lists' heads while
    /// they are cells of their own (`log.on`: a machine's `Rest`,
    /// `pdf::WithoutCells`).
    pub(crate) fn hash_without_heads<S: core::hash::Hasher>(&self, h: &mut S) {
        use core::hash::Hash;
        if !self.log.on {
            self.hash(h);
        } else if self.virt {
            self.symbolic.hash(h);
        } else {
            (self.tab.len(), self.obj_ptr, self.symbolic).hash(h);
        }
    }

    /// `sys_obj_ptr`: the last object, object streams included.
    pub(crate) fn sys_obj_ptr(&self) -> i32 {
        i32::try_from(self.tab.len() - 1).unwrap_or(i32::MAX)
    }

    /// The number of entries (object 0 included).
    pub(crate) fn len(&self) -> usize {
        if self.virt {
            self.vtab.len()
        } else {
            self.tab.len()
        }
    }

    /// A walk along the list of type `t` ([`Walk`]).
    pub(crate) fn walk(&self, t: usize) -> Walk {
        Walk {
            at: self.head(t),
            started: false,
            left: self.len(),
        }
    }

    /// The `OBJS` field's version: the entries' (as made), the lists'
    /// heads, `obj_ptr`. (Virtual numbers are a machine's, which records
    /// no versions: they are only counted.)
    pub(crate) fn table_version(&self) -> u128 {
        self.version_with(self.tab.version())
    }

    /// [`Self::table_version`], every entry hashed again (check mode).
    pub(crate) fn table_content_version(&self) -> u128 {
        self.version_with(self.tab.content_version())
    }

    fn version_with(&self, tab: u128) -> u128 {
        use partex_ssa::Version;
        if self.ssa.on {
            // (the entries are slots of their own, and how many there are
            // is the link's: the lists' heads)
            return Version::node(0x6f62_6a73, &[Version::of(&(self.head, self.symbolic))]).0;
        }
        let rest = if self.virt {
            Version::of(&(self.vtab.len(), self.alog.len(), self.vseed, self.vcount))
        } else {
            Version(0)
        };
        let fixed = Version::of(&(self.head, self.obj_ptr, self.symbolic, self.virt));
        Version::node(0x6f62_6a73, &[Version(tab), fixed, rest]).0
    }

    /// The `OBJ_TREES` field's version: the trees'.
    pub(crate) fn trees_version(&self) -> u128 {
        use partex_ssa::Version;
        if self.ssa.on {
            // (each entry is a slot of its own)
            return Version::node(0x7472_6565, &[]).0;
        }
        let parts: Vec<Version> = self.trees.iter().map(|t| Version(t.version())).collect();
        Version::node(0x7472_6565, &parts).0
    }

    #[inline]
    pub(crate) fn get(&self, k: i32) -> &Entry {
        if self.log.on && !self.log.objs.last_is(|&x| x == k) && !self.log.objs.push(k) {
            self.log.overflow.set(true);
        }
        if self.virt {
            // (SSA mode: one a run found where a later definition is, as
            // `get_mut`)
            return self
                .vtab
                .get(&k)
                .or_else(|| self.vtab.get(&0).filter(|_| self.ssa.on))
                .expect("a virtual object");
        }
        &self.tab[usize::try_from(k).unwrap_or(0)]
    }

    /// Entry `k`, to change (a machine's region reads it, then writes it).
    pub(crate) fn get_mut(&mut self, k: i32) -> &mut Entry {
        if self.ssa.on && !self.ssa.before.contains_key(&k) {
            let v = self.cell_version(k);
            self.ssa.before.insert(k, v);
        }
        if self.log.on && !(self.log.objs.push(k) && self.log.objs.push(-1 - k)) {
            self.log.overflow.set(true);
        }
        if self.virt {
            if self.ssa.on && !self.vtab.contains_key(&k) {
                // (SSA mode: an object a run found where a later
                // definition is, not placed; the run is dropped and made
                // again with it placed)
                let e = self.vtab.get(&0).cloned().expect("object 0");
                self.vtab.insert(k, e);
            }
            return self.vtab.get_mut(&k).expect("a virtual object");
        }
        &mut self.tab[usize::try_from(k).unwrap_or(0)]
    }

    /// Turn virtual numbers on (`vnum.rs`; before any object is made).
    pub(crate) fn set_virt(&mut self, on: bool) {
        self.virt = on;
        if on {
            self.vtab.insert(0, self.tab[0].clone());
        }
    }

    /// Take `other`'s numbering cache (with its log).
    pub(crate) fn take_ncache(&mut self, other: &Self) {
        self.ncache = other.ncache.clone();
    }

    /// Record a numbering event (virtual numbers).
    pub(crate) fn num_event(&mut self, e: super::vnum::NumEvent) {
        if self.ssa.on {
            self.ssa.events.push(e);
        } else {
            self.alog.push(e);
        }
    }

    /// pdfTeX's numbering where the job is now, observed: the region
    /// reads `MCell::Numbering` (virtual numbers only).
    pub(crate) fn numbering(&mut self) -> Arc<super::vnum::Numbering> {
        self.numbering_whole(true)
    }

    /// [`Self::numbering`], counted as a whole reading if `whole` (and
    /// then read whole: `forced`; a single number read with `NUM_ANSWERS`
    /// on is an answer instead).
    fn numbering_whole(&mut self, whole: bool) -> Arc<super::vnum::Numbering> {
        if self.ssa.on {
            return self.ssa_numbering();
        }
        if whole {
            self.log.observers.2 += 1;
        }
        if whole || !self.answers_on() {
            self.log.forced = true;
        }
        let mut n = self.ncache.0.take().unwrap_or_default();
        if n.seen > self.alog.len() {
            // (the log was set back: a restored state)
            n = Arc::default();
        }
        if n.seen < self.alog.len() {
            self.alog.catch_up(Arc::make_mut(&mut n));
        }
        self.ncache.0 = Some(n.clone());
        n
    }

    /// pdfTeX's number of object `k` (observed; `k` itself without
    /// virtual numbers).
    pub(crate) fn final_num(&mut self, k: i32) -> i32 {
        if !self.virt {
            return k;
        }
        self.log.observers.0 += 1;
        self.log.observers.3.insert(k);
        let n = self.numbering_whole(false).of(k);
        if self.answers_on() && !self.ssa.on {
            self.log.answers.push(NumAnswer::Final(k, n));
            self.log.answered_at = self.alog.len();
        }
        n
    }

    /// Whether single numbers read are answers (`NUM_ANSWERS`, and a
    /// machine's log on), not a whole reading.
    fn answers_on(&self) -> bool {
        self.log.on && NUM_ANSWERS.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// The numbering as far as it is cached, a prefix of the log (for the
    /// answers below, which do not change the cache).
    fn cached_numbering(&self) -> Arc<super::vnum::Numbering> {
        match &self.ncache.0 {
            Some(n) if n.seen <= self.alog.len() => n.clone(),
            _ => Arc::default(),
        }
    }

    /// What `final_num(k)` would answer now, unobserved (a machine's
    /// `MCell::FinalNum`).
    pub(crate) fn peek_final_num(&self, k: i32) -> i32 {
        self.alog.final_of(&self.cached_numbering(), k)
    }

    /// What `of_final(n)` would answer now, unobserved (a machine's
    /// `MCell::OfFinal`).
    pub(crate) fn peek_of_final(&self, n: i32) -> Option<i32> {
        self.alog.vid_of(&self.cached_numbering(), n)
    }

    /// The object pdfTeX numbers `n` (observed; `n` itself without
    /// virtual numbers), if any.
    pub(crate) fn of_final(&mut self, n: i32) -> Option<i32> {
        if self.virt {
            self.log.observers.1 += 1;
            self.log.observers.3.insert(-n);
            let v = self.numbering_whole(false).vid.get(&n).copied();
            if self.answers_on() && !self.ssa.on {
                self.log.answers.push(NumAnswer::Of(n, v));
                self.log.answered_at = self.alog.len();
            }
            v
        } else {
            Some(n)
        }
    }

    /// The objects, in pdfTeX's order (virtual: by the numbering, which
    /// is observed).
    pub(crate) fn numbers(&mut self) -> Vec<i32> {
        if self.virt {
            let n = self.numbering();
            (0..=n.sys)
                .map(|k| n.vid.get(&k).copied().unwrap_or(-k))
                .collect()
        } else {
            (0..i32::try_from(self.tab.len()).unwrap_or(0)).collect()
        }
    }

    /// Every object's key, in no particular order (a machine's).
    pub(crate) fn keys(&self) -> Vec<i32> {
        if self.virt {
            self.vtab.keys()
        } else {
            (0..i32::try_from(self.tab.len()).unwrap_or(0)).collect()
        }
    }

    /// A new virtual id: from `name` (an identity an edit does not move)
    /// or else the step's position and count, the first free one (reading
    /// each tried).
    fn new_vid(&mut self, name: Option<u128>) -> i32 {
        if self.ssa.on {
            return self.ssa_vid(name);
        }
        let mut j = 0u32;
        loop {
            let h = if let Some(n) = name {
                j += 1;
                partex_engine::stablehash::StableHasher::of(&(n, j))
            } else {
                self.vcount += 1;
                partex_engine::stablehash::StableHasher::of(&(self.vseed, self.vcount - 1))
            };
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            let v = 1 + (h as u32 % 0x7fff_fffe) as i32;
            if self.log.on && !self.log.objs.push(v) {
                self.log.overflow.set(true);
            }
            if !self.vtab.contains_key(&v) {
                return v;
            }
        }
    }

    /// The destination names, in order of creation (a machine's region
    /// reads `MCell::Dests`).
    pub(crate) fn dest_names_read(&mut self) -> Vec<(Arc<[u8]>, i32)> {
        self.log.dests_read = true;
        if self.ssa.on {
            // (SSA mode keeps no list of them: the destinations' list,
            // newest first, has them)
            let mut v = Vec::new();
            let mut w = self.walk(OBJ_TYPE_DEST);
            while let Some(k) = w.next(self) {
                if let Id::Name(s) = &self.get(k).info {
                    v.push((s.clone(), k));
                }
            }
            v.reverse();
            return v;
        }
        self.dest_names.to_vec()
    }

    /// How many destination names there are (read as
    /// [`Self::dest_names_read`]).
    pub(crate) fn dest_count(&mut self) -> usize {
        if self.ssa.on {
            return self.dest_names_read().len();
        }
        self.log.dests_read = true;
        self.dest_names.len()
    }

    /// Add destination name `n` for object `k`.
    fn push_dest(&mut self, n: Arc<[u8]>, k: i32) {
        use core::hash::{Hash, Hasher};
        let mut h = partex_engine::stablehash::StableHasher::new();
        h.write_u128(self.dest_hash);
        (&n, k).hash(&mut h);
        self.dest_hash = h.finish128();
        self.dest_names.push((n, k));
    }

    /// Add destination names (a machine's `MCell::Dests`, which
    /// accumulates).
    pub(crate) fn add_dests(&mut self, d: &[(Arc<[u8]>, i32)]) {
        for (n, k) in d {
            self.push_dest(n.clone(), *k);
        }
    }

    /// Take `other`'s destination names (a machine's `Rest` set over a
    /// state: they are `MCell::Dests`'s).
    pub(crate) fn take_dests_from(&mut self, other: &Self) {
        self.dest_names.clone_from(&other.dest_names);
        self.dest_hash = other.dest_hash;
    }

    /// Make room in the log for a region (a machine's).
    pub(crate) fn reserve_log(&mut self) {
        if self.log.on {
            // (drained after each step)
            let n = (4 * self.len()).max(1 << 14);
            if core::mem::replace(&mut self.log.sized, true) {
                self.log.objs.reserve(n);
            } else {
                // (a fresh log's size, in the room `adopt_log` kept)
                self.log.objs.resize(n);
            }
        }
    }

    /// Take `old`'s log room, emptied, for this table's fresh log (a
    /// machine's restore replaces the engine).
    pub(crate) fn adopt_log(&mut self, old: &mut Self) {
        if self.log.sized || self.log.objs.len() > 0 {
            return;
        }
        self.log.objs = core::mem::take(&mut old.log.objs);
        self.log.objs.clear();
    }

    /// Take what the region did (a machine's): the entries read and
    /// written in order, whether the log overflowed, the name cells.
    pub(crate) fn take_log(&mut self) -> (Vec<i32>, bool, Vec<(bool, NameCell)>) {
        let objs = self.log.objs.to_vec();
        self.log.objs.clear();
        let overflow = self.log.overflow.get();
        self.log.overflow.set(false);
        (objs, overflow, core::mem::take(&mut self.log.names))
    }

    /// Entry `k` (not logged: a machine's).
    pub(crate) fn entry(&self, k: i32) -> Option<&Entry> {
        if self.virt {
            self.vtab.get(&k)
        } else {
            self.tab.get(usize::try_from(k).ok()?)
        }
    }

    /// Hash the lookup trees and destination names (a machine's digest).
    pub(crate) fn hash_names<S: core::hash::Hasher>(&self, h: &mut S) {
        use core::hash::Hash;
        (&self.trees, &self.dest_names).hash(h);
    }

    /// Lookup tree `t`'s entry for `i`.
    pub(crate) fn tree_entry(&self, t: usize, i: &Id) -> Option<i32> {
        self.trees.get(t)?.get(i).copied()
    }

    /// Set lookup tree `t`'s entry for `i` (a machine's cell).
    pub(crate) fn set_tree_entry(&mut self, t: usize, i: &Id, v: Option<i32>) {
        if let Some(tree) = self.trees.get_mut(t) {
            match v {
                Some(k) => {
                    tree.insert(i.clone(), k);
                }
                None => {
                    tree.remove(i);
                }
            }
        }
    }

    /// Set entry `k` (a machine's cell): `None` takes it away (and every
    /// entry after it: the table has no holes).
    pub(crate) fn set_entry(&mut self, k: i32, e: Option<Entry>) {
        if self.virt {
            match e {
                Some(e) => {
                    self.vtab.insert(k, e);
                }
                None => {
                    self.vtab.remove(&k);
                }
            }
            return;
        }
        let k = usize::try_from(k).unwrap_or(0);
        match e {
            Some(e) => {
                while self.tab.len() <= k {
                    self.tab.push(Entry {
                        info: Id::Num(0),
                        link: 0,
                        offset: -2,
                        os_idx: 0,
                        aux: Aux::None,
                    });
                }
                self.tab[k] = e;
            }
            None => self.tab.truncate(k),
        }
    }

    /// Take `other`'s cells (entries, trees), keeping the rest (a
    /// machine's `Rest` set over a state).
    pub(crate) fn take_cells_from(&mut self, other: &Self) {
        self.tab.clone_from(&other.tab);
        self.vtab.clone_from(&other.vtab);
        self.trees.clone_from(&other.trees);
    }

    /// Entry `k`'s version: what a machine compares it by (with symbolic
    /// object streams, a written object as written, not where).
    pub(crate) fn entry_version(&self, e: &Entry) -> u128 {
        use core::hash::Hash;
        let mut h = partex_engine::stablehash::StableHasher::new();
        if self.symbolic && e.offset >= 0 {
            (&e.info, e.link, -3i64, e.os_idx == -1, &e.aux).hash(&mut h);
        } else {
            e.hash(&mut h);
        }
        h.finish128()
    }

    /// Object `k`'s slot's version (SSA mode): its entry's, a written
    /// object as written, not where.
    pub(crate) fn cell_version(&self, k: i32) -> u128 {
        self.vtab
            .get(&k)
            .map_or(ABSENT_CELL, |e| self.entry_version(e))
    }

    /// The version of the lookup trees' entry at slot `key` (SSA mode).
    pub(crate) fn name_version(&self, key: i64) -> u128 {
        let Some((t, i)) = self.ssa.name_keys.get(&key) else {
            return ABSENT_CELL;
        };
        let v = self.trees.get(usize::from(*t)).and_then(|m| m.get(i));
        partex_ssa::Version::of(&(b"name", v)).0
    }

    /// The version of step `n`'s numbering events (SSA mode).
    pub(crate) fn num_version(&self, n: u32) -> u128 {
        self.ssa.steplogs.get(&n).map_or(ABSENT_CELL, |e| e.1)
    }

    /// A lookup tree's entry read (`write`: written), SSA mode.
    fn ssa_name(&mut self, write: bool, t: u8, i: &Id) {
        let key = name_slot(t, i);
        self.ssa
            .name_keys
            .entry(key)
            .or_insert_with(|| (t, i.clone()));
        let v = if write { 0 } else { self.name_version(key) };
        self.ssa.names.push((write, key, v));
    }

    /// The lookup trees' entry at slot `key`: its type, identifier and
    /// object (a record's value, SSA mode).
    pub(crate) fn name_cell(&self, key: i64) -> Option<(u8, Id, Option<i32>)> {
        let (t, i) = self.ssa.name_keys.get(&key)?;
        let v = self.trees.get(usize::from(*t))?.get(i).copied();
        Some((*t, i.clone(), v))
    }

    /// Put the lookup trees' entry back (a record's value, SSA mode).
    pub(crate) fn set_name_cell(&mut self, key: i64, t: u8, i: &Id, v: Option<i32>) {
        self.ssa
            .name_keys
            .entry(key)
            .or_insert_with(|| (t, i.clone()));
        self.set_tree_entry(usize::from(t), i, v);
    }

    /// Take the lookup trees' entry at slot `key` away (SSA mode).
    pub(crate) fn clear_name_cell(&mut self, key: i64) {
        if let Some((t, i)) = self.ssa.name_keys.get(&key).cloned() {
            self.set_tree_entry(usize::from(t), &i, None);
        }
    }

    /// A new virtual id in SSA mode: an object TeX names by `name`, the
    /// next of the open step's otherwise.
    fn ssa_vid(&mut self, name: Option<u128>) -> i32 {
        use partex_engine::stablehash::StableHasher;
        let owner = if let Some(n) = name {
            n
        } else {
            let c = self.vcount;
            self.vcount += 1;
            if let Some(step) = self.ssa.step
                && step < (1 << (30 - SEQ_BITS)) - 1
                && c < 1 << SEQ_BITS
            {
                return 1 + i32::try_from(step << SEQ_BITS | c).unwrap_or(0);
            }
            StableHasher::of(&(b"seq", self.ssa.seed, self.ssa.step, c))
        };
        let mut j = 0u32;
        loop {
            let h = StableHasher::of(&(owner, j));
            j += 1;
            #[allow(clippy::cast_possible_truncation, reason = "a hash's low bits")]
            let low = (h as u32) % (HASHED.cast_unsigned() - 1);
            let v = HASHED + low.cast_signed();
            match self.ssa.vowner.get(&v) {
                Some(&o) if o != owner => {}
                _ => {
                    self.ssa.vowner.insert(v, owner);
                    return v;
                }
            }
        }
    }

    /// An applied call begins (SSA mode): the objects it makes are named
    /// by its name; what it replaces, given back at its end.
    pub(crate) fn ssa_call_seed(&mut self, name: u128) -> (Option<u32>, u128, u32) {
        let was = (self.ssa.step, self.ssa.seed, self.vcount);
        if self.ssa.on {
            self.ssa.step = None;
            self.ssa.seed = name;
            self.vcount = 0;
        }
        was
    }

    /// The applied call ends (SSA mode).
    pub(crate) fn ssa_call_seed_end(&mut self, was: (Option<u32>, u128, u32)) {
        if self.ssa.on {
            (self.ssa.step, self.ssa.seed, self.vcount) = was;
        }
    }

    /// The numbering of the steps before the open one, and the slots read
    /// for it (SSA mode).
    pub(crate) fn ssa_set_prefix(
        &mut self,
        n: Arc<super::vnum::Numbering>,
        reads: Arc<[(u32, u128)]>,
    ) {
        self.ssa.prefix = Some((n, reads));
        self.ncache = NumCache::default();
    }

    /// Note a read of entry `k` made without [`Self::get`] (SSA mode).
    pub(crate) fn log_read(&self, k: i32) {
        if self.log.on && !self.log.objs.push(k) {
            self.log.overflow.set(true);
        }
    }

    /// A step begins (SSA mode): its objects are named by `step`.
    pub(crate) fn ssa_step_begin(&mut self, step: Option<u32>) {
        self.ssa.step = step;
        self.ssa.seed = u128::from(step.unwrap_or(u32::MAX));
        self.vcount = 0;
        self.ssa.events.clear();
        self.ssa.prefix = None;
        self.ncache = NumCache::default();
        self.ssa.before.clear();
        self.ssa.names.clear();
        self.log.objs.clear();
        self.log.overflow.set(false);
        self.reserve_log();
    }

    /// The numbering where the job is now (SSA mode): the steps' before
    /// the open one (`ssa.prefix`, made by `Tex::obj_observe`), then the
    /// open step's events.
    fn ssa_numbering(&mut self) -> Arc<super::vnum::Numbering> {
        let base = self
            .ssa
            .prefix
            .as_ref()
            .map_or_else(Arc::default, |p| p.0.clone());
        let want = base.seen + self.ssa.events.len();
        if let Some(n) = &self.ncache.0
            && n.seen == want
        {
            return n.clone();
        }
        let mut n = (*base).clone();
        for &e in &self.ssa.events {
            n.step(e);
        }
        let n = Arc::new(n);
        self.ncache.0 = Some(n.clone());
        n
    }

    /// pdfTeX §698: `pdf_create_obj`: a new object of type `t` with
    /// identifier `i` (the caller checks the table size).
    pub(crate) fn create(&mut self, t: usize, i: Id) -> i32 {
        let name = tex_identity(t, &i);
        self.create_named(t, i, name)
    }

    /// [`Self::create`], a virtual id named by `name` if given (see
    /// [`tex_identity`]).
    pub(crate) fn create_named(&mut self, t: usize, i: Id, name: Option<u128>) -> i32 {
        let fresh = Entry {
            info: i.clone(),
            link: 0,
            offset: -2,
            os_idx: 0,
            aux: Aux::None,
        };
        let k = if self.virt {
            let v = self.new_vid(name);
            self.vtab.insert(v, fresh);
            self.num_event(super::vnum::NumEvent::Create(v));
            v
        } else {
            self.tab.push(fresh);
            self.sys_obj_ptr()
        };
        self.obj_ptr = k;
        if self.log.on && !self.log.objs.push(-1 - k) {
            self.log.overflow.set(true);
        }
        let tree = u8::try_from(t).unwrap_or(u8::MAX);
        if self.ssa.on {
            self.ssa_name(false, tree, &i);
        } else if self.log.on {
            self.log
                .names
                .push((false, NameCell::Tree(tree, i.clone())));
        }
        if !self.trees[t].contains_key(&i) {
            self.trees[t].insert(i.clone(), k);
            if self.ssa.on {
                self.ssa_name(true, tree, &i);
            } else if self.log.on {
                self.log.names.push((true, NameCell::Tree(tree, i.clone())));
            }
        }
        if t == OBJ_TYPE_PAGE {
            // pages are kept in decreasing order of their numbers
            let n = i.num();
            let mut p = self.head(t);
            if p == 0 || self.get(p).info.num() < n {
                self.get_mut(k).link = p;
                self.set_head(t, k);
            } else {
                let mut q = p;
                let mut left = self.len();
                while p != 0 && left > 0 {
                    if self.get(p).info.num() < n {
                        break;
                    }
                    q = p;
                    p = self.get(p).link;
                    // (a list that never ends: [`Walk`])
                    left -= 1;
                }
                self.get_mut(q).link = k;
                self.get_mut(k).link = p;
            }
        } else if t != OBJ_TYPE_OTHERS {
            self.get_mut(k).link = self.head(t);
            self.set_head(t, k);
            if t == OBJ_TYPE_DEST
                && !self.ssa.on
                && let Id::Name(s) = i
            {
                self.push_dest(s, k);
            }
        }
        k
    }

    /// avlstuff.c's `avlfindobj`: the object of type `t` with
    /// identifier `i`, or 0.
    pub(crate) fn find(&mut self, t: usize, i: &Id) -> i32 {
        if self.ssa.on {
            self.ssa_name(false, u8::try_from(t).unwrap_or(u8::MAX), i);
        } else if self.log.on {
            let tree = u8::try_from(t).unwrap_or(u8::MAX);
            self.log
                .names
                .push((false, NameCell::Tree(tree, i.clone())));
        }
        self.trees[t].get(i).copied().unwrap_or(0)
    }

    /// pdfTeX §1546: `pdf_check_obj`: whether object `n` is on the list
    /// of type `t`.
    pub(crate) fn on_list(&self, t: usize, n: i32) -> bool {
        let mut w = self.walk(t);
        while let Some(k) = w.next(self) {
            if k == n {
                return true;
            }
        }
        false
    }

    pub(crate) fn is_scheduled(&self, k: i32) -> bool {
        self.get(k).offset > -2
    }

    pub(crate) fn is_written(&self, k: i32) -> bool {
        self.get(k).offset > -1
    }

    pub(crate) fn set_scheduled(&mut self, k: i32) {
        if self.get(k).offset == -2 {
            self.get_mut(k).offset = -1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run of a step that read entries at a later definition (the
    /// thesis's cold trip 2): font `a`'s entry, as a later run left it,
    /// links to font `b`, which the step's head reaches, and the step
    /// makes `a` again, pushing it on the list it is on: the walks end.
    #[test]
    fn walk_ends_on_a_list_that_never_ends() {
        let mut t = ObjTab::default();
        let a = t.create(OBJ_TYPE_FONT, Id::Num(1));
        let b = t.create(OBJ_TYPE_FONT, Id::Num(2));
        let mut e = t.get(a).clone();
        e.link = b;
        t.set_entry(a, Some(e));
        assert!(t.on_list(OBJ_TYPE_FONT, b));
        assert!(!t.on_list(OBJ_TYPE_FONT, 99));
        let mut w = t.walk(OBJ_TYPE_FONT);
        let mut n = 0;
        while w.next(&t).is_some() {
            n += 1;
        }
        assert_eq!(n, t.len());
        // a page made on such a list is placed
        let p = t.create(OBJ_TYPE_PAGE, Id::Num(1));
        let mut e = t.get(p).clone();
        e.link = p;
        t.set_entry(p, Some(e));
        t.create(OBJ_TYPE_PAGE, Id::Num(0));
        // and a list made as pdfTeX makes it is walked whole
        let mut t = ObjTab::default();
        let ks: Vec<i32> = (0..5)
            .map(|i| t.create(OBJ_TYPE_FONT, Id::Num(i)))
            .collect();
        let mut w = t.walk(OBJ_TYPE_FONT);
        let mut seen = Vec::new();
        while let Some(k) = w.next(&t) {
            seen.push(k);
        }
        seen.reverse();
        assert_eq!(seen, ks);
    }
}
