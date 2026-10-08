//! `PARTEX_PURE=FILE`: the pure SSA tracer (DESIGN 3.17, "Measured"), an
//! observer of a plain build that counts what the build would be as a
//! pure graph: a node per command, per expandable primitive, per recorded
//! call and per `build_page`, each reading the definitions that reach it
//! and defining what it writes. It changes nothing the engine does.
//!
//! The model it measures:
//! - *Nodes.* A main-control command (from its expansion to the next
//!   `reswitch`: a word is one node), an expandable primitive's expansion,
//!   a recorded call (`hpack`, `vpack`, `line_break`, a page step,
//!   `ship_out`, `write_out`, the fonts), `build_page`. A macro call is a
//!   step, not a node: its meaning is a read of the node that runs it.
//! - *Edges.* A read of an address resolves to the definition reaching it
//!   (the last write, scope-aware: a group's end makes no definition, the
//!   address's definition is again the one saved at the group's start).
//!   A read of an address the node itself wrote is internal.
//! - *Structure is not data.* The save stack, the nest's depth, the
//!   conditionals' stack, the hash's chains and allocators, `align_state`
//!   and the log's columns are the builder's bookkeeping: reads of them
//!   are counted, not edges. (The meaning a lookup reaches is an eqtb
//!   read, an edge; a mode is a list field, an edge.)
//! - *Appends do not read.* An append to the current list (`tail_append`
//!   and its kin) is not a read of the list; a read of the whole list
//!   (the line breaker's, a pack's, `\lastbox`'s) depends on every
//!   append since the list was last taken.
//! - *Conditionals.* The nodes of a taken arm depend on the test; at
//!   `\fi` each address the arm defined gets a φ.
//! - *Classes.* A node is of the page chain if it is `build_page`, a page
//!   step, `ship_out`, the output routine's call, or a command run while
//!   the output routine is active (or nested in one of these); else of
//!   the body.
//! - *Field reads.* A recorded call that reads a list's fields
//!   (`Tracker::pure_fields`) is keyed by them, not by the list.
//!
//! What it reports (JSON in FILE): nodes by kind; reads and writes; φs;
//! the critical path in nodes and in events (each node costs one plus its
//! reads and writes) against the total; the page chain's own path (only
//! page-chain nodes' costs counted along any path) against the page
//! chain's total; how many body nodes read a definition the page chain
//! made, directly and transitively, and the addresses they read; the
//! graph's projected memory, unfolded and folded by the fold rule of
//! DESIGN 3.17 ("Folding"). `PARTEX_PURE_TRACE=PATH` also writes a
//! record per node (`PATH.nodes`: kind u16, class u8, flags u8, source
//! hash u32, operand hash u32) and the line breaker's lines (`PATH.lines`)
//! for `scripts/pure-diff.py`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Write as _;

use partex_core::Tex;
use partex_core::host::Host;
use partex_core::ssa::{EngineView, Func};
use partex_core::track::{Cell, Row, Tracker, list, scalar};

const NONE: u32 = u32::MAX;
const BODY: u8 = 0;
const PAGE: u8 = 1;
/// Kinds: a command is its `cur_cmd`; these are offsets.
const K_EXPAND: u16 = 256;
const K_CALL: u16 = 512;
const K_BUILD_PAGE: u16 = 768;
const K_SETUP: u16 = 1023;
const K_PENDING: u16 = u16::MAX;
/// A conditional's test and its `\else`/`\fi` make no tokens: what
/// they decide is the arm, an operand of the nodes in it.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "command codes"
)]
const IF_TEST_CMD: u16 = partex_engine::web::IF_TEST as u16;
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "command codes"
)]
const FI_OR_ELSE_CMD: u16 = partex_engine::web::FI_OR_ELSE as u16;
/// The critical path's edges that are not an address's: a conditional's
/// test, a token list's maker, a nested node's result.
const COND_KEY: u64 = u64::MAX;
const LEVEL_KEY: u64 = u64::MAX - 1;
const NESTED_KEY: u64 = u64::MAX - 2;
/// The origin of a page-chain node's own definitions.
const PAGE_ORIGIN: u64 = u64::MAX - 3;
/// Eqtb locations of the registers above 255 start here (`xregs.rs`).
const EXT_BASE: i32 = 0x2000_0000;

/// Distances along the longest path into a definition: in nodes, in
/// events, and in page-chain events only.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Dist {
    n: u64,
    w: u64,
    p: u64,
}

impl Dist {
    fn max(self, o: Dist) -> Dist {
        Dist {
            n: self.n.max(o.n),
            w: self.w.max(o.w),
            p: self.p.max(o.p),
        }
    }
}

#[derive(Clone, Copy)]
struct Def {
    prod: u32,
    d: Dist,
    page: bool,
    /// Downstream of the page chain (a path from a page-chain node).
    after_page: u64,
    fold: u32,
    fpos: u32,
    /// Its version, when known (0: not yet).
    ver: u64,
}

impl Default for Def {
    fn default() -> Def {
        Def {
            prod: NONE,
            d: Dist::default(),
            page: false,
            after_page: 0,
            fold: NONE,
            fpos: 0,
            ver: 0,
        }
    }
}

/// The appends to a list since it was last taken.
#[derive(Clone, Copy, Default)]
struct App {
    d: Dist,
    page: bool,
    after_page: u64,
    last: Option<Def>,
}

#[derive(Clone, Default)]
struct Addr {
    def: Def,
    reader: u32,
    writer: u32,
    reader_fold: u32,
    writer_fold: u32,
    app: Option<Box<App>>,
    /// The two definitions before this one (value numbering).
    hist: [Def; 2],
}

impl Addr {
    fn new() -> Addr {
        Addr {
            reader: NONE,
            writer: NONE,
            reader_fold: NONE,
            writer_fold: NONE,
            ..Addr::default()
        }
    }
}

#[derive(Clone, Copy)]
struct Read {
    key: u64,
    prev_reader: u32,
    prod: u32,
    page: bool,
    after_page: u64,
    from_fold: u32,
    from_fpos: u32,
    ver: u64,
    list: bool,
}

struct Frame {
    id: u32,
    kind: u16,
    class: u8,
    command: bool,
    events: u64,
    d_in: Dist,
    after_page: u64,
    /// What first made the node downstream of the page chain.
    via: u64,
    /// The operand on its longest path in: the node and the address.
    pred: (u32, u64),
    reads: Vec<Read>,
    writes: Vec<u64>,
    appends: Vec<u64>,
    append_key: Option<u64>,
    src: u64,
    toks: u64,
    last_child: u32,
    arg: u64,
    fields: Option<u64>,
    unversioned: bool,
    lines: Vec<u32>,
}

struct CondE {
    opener: u32,
    opener_kind: u16,
    test: Option<(Dist, u64)>,
    written: Vec<u64>,
}

#[derive(Default)]
struct Kind {
    nodes: u64,
    events: u64,
    reads: u64,
    writes: u64,
}

#[derive(Default)]
struct State {
    eqtb: Vec<Addr>,
    other: HashMap<u64, Addr>,
    frames: Vec<Frame>,
    next_id: u32,
    pending_append: bool,
    /// The calls open, each a frame (true) or not (the output routine's).
    calls: Vec<bool>,
    conds: Vec<CondE>,
    saved: HashMap<i32, Def>,
    /// Value numbering (`PARTEX_PURE_VN=1`): a definition equal to one
    /// of the address's two before it is that one.
    vn: bool,
    /// `PARTEX_PURE_DEBUG=KEY`: every event of that address, on stderr.
    debug: Option<u64>,
    /// `PARTEX_PURE_SPEC_TAIL=1`: a body node's read of the main vertical
    /// list's tail (the contributions whole, or the page builder's
    /// `last_glue` and friends: `\lastskip`, `\lastpenalty`, …) is
    /// predicted (the last contribution, no fire since), not a page-chain
    /// operand.
    spec_tail: bool,
    body_tail_reads: u64,
    /// What made each input level's token list: its node's operands.
    levels: Vec<(Dist, u64, u32)>,
    /// `PARTEX_PURE_PATH=1`: each node's predecessor on its longest path
    /// in, by node id, and the node the critical path ends at.
    path: Option<Vec<(u32, u64)>>,
    crit_end: u32,
    kinds: HashMap<u16, Kind>,
    macros: u64,
    phis: u64,
    conditionals: u64,
    struct_reads: u64,
    struct_writes: u64,
    total: Dist,
    crit: Dist,
    page_nodes: u64,
    page_events: u64,
    body_nodes: u64,
    body_page_direct: u64,
    body_page_trans: u64,
    body_page_addrs: HashMap<u64, u64>,
    /// Body nodes downstream of the page chain by what carried it: an
    /// address read (key), a conditional's test (`u64::MAX`), a token
    /// list read (`u64::MAX - 1`), a nested node (`u64::MAX - 2`).
    body_after_via: HashMap<u64, u64>,
    ext_reads: u64,
    net_writes: u64,
    list_reads: u64,
    appends: u64,
    unversioned_nodes: u64,
    // folding
    fold_id: u32,
    fold_class: u8,
    fold_last: u32,
    fold_len: Vec<u32>,
    cuts: HashSet<u64>,
    fold_reads: u64,
    fold_writes: u64,
    anomalies: u64,
    cond_mismatch: u64,
    class_changes: u64,
    /// The work and the path before the first `\shipout` (the setup).
    setup: Option<(Dist, Dist)>,
    trace: Option<(
        std::io::BufWriter<std::fs::File>,
        std::io::BufWriter<std::fs::File>,
    )>,
}

fn mix(h: u64, x: u64) -> u64 {
    (h.rotate_left(5) ^ x).wrapping_mul(0x517c_c1b7_2722_0a95)
}

fn fold128(v: u128) -> u64 {
    // (the two halves of a version)
    #[allow(clippy::cast_possible_truncation, reason = "the low half, mixed")]
    let lo = v as u64;
    mix(lo, (v >> 64) as u64)
}

/// What an address is to the model.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Struct,
    Normal,
    List,
}

fn row_key(row: Row) -> (u64, Class) {
    let k = |tag: u64, x: u64| (tag << 56) | (x & 0x00ff_ffff_ffff_ffff);
    let n = |x: u32| u64::from(x);
    match row {
        Row::Str(i) => (k(1, i as u64), Class::Struct),
        Row::Scalar(s) => {
            let c = match s {
                scalar::STR_TOP
                | scalar::HASH_USED
                | scalar::HASH_HIGH
                | scalar::SHOWN_MODE
                | scalar::ALIGN_STATE
                | scalar::TERM_OFFSET
                | scalar::FILE_OFFSET
                | scalar::GLUE_LINEAGE
                | scalar::OPEN_PARENS
                | scalar::SYNCTEX_TAGS
                | scalar::SYNCTEX_FLAGS
                // (`\afterassignment`'s token: input pending, step state)
                | scalar::AFTER_TOKEN => Class::Struct,
                _ => Class::Normal,
            };
            (k(2, u64::from(s)), c)
        }
        Row::Cond => (k(3, 0), Class::Struct),
        Row::Mark(i) => (k(4, n(i)), Class::Normal),
        Row::List(i) => {
            let f = i % list::STRIDE;
            let c = if f == u32::from(list::LIST) || f == u32::from(list::MLIST) {
                Class::List
            } else {
                Class::Normal
            };
            (k(5, n(i)), c)
        }
        Row::Nest => (k(6, 0), Class::Struct),
        Row::Align(f) => (k(7, u64::from(f)), Class::Normal),
        Row::Page(f) => (k(8, u64::from(f)), Class::Normal),
        Row::Save(i) => (k(9, n(i)), Class::Struct),
        Row::Font(i) => (k(10, n(i)), Class::Normal),
        Row::FontTable => (k(11, 0), Class::Normal),
        Row::Hyph(i) => (k(12, n(i)), Class::Normal),
        Row::Pdf(f) => (k(13, u64::from(f)), Class::Normal),
        Row::Dvi(f) => (k(14, u64::from(f)), Class::Normal),
        Row::Glyphs(i) => (k(15, n(i)), Class::Normal),
        Row::PageNode(i) => (k(16, n(i)), Class::Normal),
        Row::Out(i) => (k(17, u64::from(i)), Class::Normal),
        Row::Read(i) => (k(18, u64::from(i)), Class::Normal),
        Row::Random => (k(19, 0), Class::Normal),
        Row::Clock => (k(20, 0), Class::Normal),
        Row::Sealed(x) => (k(21, mix(x, 21)), Class::Normal),
        Row::PdfObj(i) => (k(22, u64::from(i.cast_unsigned())), Class::Normal),
        Row::PdfName(i) => (k(23, mix(i.cast_unsigned(), 23)), Class::Normal),
        Row::PdfNum(i) => (k(24, n(i)), Class::Normal),
    }
}

/// Whether `key` is the main vertical list's tail as the body reads it:
/// the outer level's list (the contributions), or the page builder's
/// `last_glue`, `last_penalty`, `last_kern`, `last_node_type`.
fn is_tail(key: u64) -> bool {
    use partex_core::track::page;
    key == row_key(Row::List(list::slot(0, list::LIST))).0
        || [
            page::LAST_GLUE,
            page::LAST_PENALTY,
            page::LAST_KERN,
            page::LAST_NODE_TYPE,
        ]
        .iter()
        .any(|&f| key == row_key(Row::Page(f)).0)
}

fn eqtb_key(p: i32) -> u64 {
    if p >= EXT_BASE {
        (25 << 56) | u64::from((p - EXT_BASE).cast_unsigned())
    } else {
        u64::from(p.cast_unsigned())
    }
}

fn key_name(key: u64) -> String {
    let tag = key >> 56;
    let x = key & 0x00ff_ffff_ffff_ffff;
    let names = [
        "eqtb",
        "str",
        "scalar",
        "cond",
        "mark",
        "list",
        "nest",
        "align",
        "page",
        "save",
        "font",
        "font_table",
        "hyph",
        "pdf",
        "dvi",
        "glyphs",
        "page_node",
        "out",
        "read",
        "random",
        "clock",
        "sealed",
        "pdf_obj",
        "pdf_name",
        "pdf_num",
        "eqtb_ext",
    ];
    let t = usize::try_from(tag).unwrap_or(0);
    format!("{}:{x}", names.get(t).copied().unwrap_or("?"))
}

impl Addr {
    /// The current definition's version is `ver`: with value numbering,
    /// a definition equal to one of the two before it is that one.
    fn known(&mut self, ver: u64, vn: bool) {
        self.def.ver = ver;
        if !vn || ver == 0 {
            return;
        }
        for i in 0..2 {
            if self.hist[i].ver == ver {
                let d = self.hist[i];
                self.hist[i] = self.def;
                self.def = d;
                return;
            }
        }
    }
}

impl State {
    /// The innermost conditional whose test is decided: the test's
    /// distance and origin, and the cause to name.
    fn cond_dep(&self) -> Option<(Dist, u64, u64, u32)> {
        self.conds.iter().rev().find_map(|c| {
            c.test.map(|t| {
                let via = if t.1 != 0 {
                    0xC0DE_0000 | u64::from(c.opener_kind)
                } else {
                    0
                };
                (t.0, t.1, via, c.opener)
            })
        })
    }

    fn addr(&mut self, key: u64) -> &mut Addr {
        if key >> 56 == 0 {
            let i = usize::try_from(key).unwrap_or(0);
            if i >= self.eqtb.len() {
                self.eqtb.resize(i + 1, Addr::new());
            }
            &mut self.eqtb[i]
        } else {
            self.other.entry(key).or_insert_with(Addr::new)
        }
    }

    fn top(&mut self) -> &mut Frame {
        if self.frames.is_empty() {
            self.open(K_SETUP, BODY, true);
        }
        self.frames.last_mut().expect("a frame")
    }

    fn open(&mut self, kind: u16, class: u8, command: bool) {
        let id = self.next_id;
        self.next_id += 1;
        let parent_class = self.frames.last().map_or(BODY, |f| f.class);
        let class = class.max(parent_class);
        // (the test of the innermost conditional decided, an operand of
        // every node in its arm; a command's when it is dispatched)
        let mut d_in = Dist::default();
        let mut after_page = 0;
        let mut via = 0;
        let mut cond_opener = NONE;
        if !command && let Some((d, a, k, o)) = self.cond_dep() {
            d_in = d;
            after_page = a;
            via = k;
            cond_opener = o;
        }
        self.frames.push(Frame {
            id,
            kind,
            class,
            command,
            events: 0,
            d_in,
            after_page,
            via,
            pred: (cond_opener, COND_KEY),
            reads: Vec::new(),
            writes: Vec::new(),
            appends: Vec::new(),
            append_key: None,
            src: 0,
            toks: 0,
            last_child: NONE,
            arg: 0,
            fields: None,
            unversioned: false,
            lines: Vec::new(),
        });
    }

    fn read(&mut self, key: u64, class: Class, ver: u64) {
        if class == Class::Struct {
            self.struct_reads += 1;
            self.top().events += 1;
            return;
        }
        let pending = std::mem::take(&mut self.pending_append);
        let f = self.top();
        f.events += 1;
        let id = f.id;
        if class == Class::List && pending {
            f.append_key = Some(key);
            return;
        }
        let vn = self.vn;
        if self.debug == Some(key) {
            let (kind, class) = self.frames.last().map_or((0, 0), |f| (f.kind, f.class));
            let d = self.addr(key).def;
            eprintln!(
                "pure: read by {id} kind {kind} class {class}: def of {} page {}",
                d.prod, d.page
            );
        }
        let a = self.addr(key);
        if a.def.prod == id || a.reader == id {
            return;
        }
        if a.def.ver == 0 && class == Class::Normal {
            a.known(ver, vn);
        }
        let prev_reader = a.reader;
        a.reader = id;
        let (mut d, mut page, mut after_page, mut src) =
            (a.def.d, a.def.page, a.def.after_page, a.def);
        let list = class == Class::List;
        if list && let Some(app) = a.app.as_deref() {
            d = d.max(app.d);
            page |= app.page;
            if after_page == 0 {
                after_page = app.after_page;
            }
            if let Some(l) = app.last {
                src = l;
            }
        }
        // (the main vertical list's tail, read by the body)
        let body = self.frames.last().is_some_and(|f| f.class == BODY);
        if body && is_tail(key) && (page || after_page != 0) {
            self.body_tail_reads += 1;
            if self.spec_tail {
                page = false;
                after_page = 0;
            }
        }
        let r = Read {
            key,
            prev_reader,
            prod: src.prod,
            page,
            // (the origin: the page-chain address read first on the way)
            after_page: if page { key } else { after_page },
            from_fold: src.fold,
            from_fpos: src.fpos,
            ver,
            list,
        };
        let f = self.frames.last_mut().expect("a frame");
        if d.w > f.d_in.w {
            f.pred = (r.prod, key);
        }
        f.d_in = f.d_in.max(d);
        if r.after_page != 0 && f.after_page == 0 {
            f.after_page = r.after_page;
            f.via = key;
        }
        f.reads.push(r);
    }

    fn write(&mut self, key: u64, class: Class) {
        if class == Class::Struct {
            self.struct_writes += 1;
            self.top().events += 1;
            return;
        }
        let f = self.top();
        f.events += 1;
        let (id, fclass) = (f.id, f.class);
        let append = class == Class::List && f.append_key == Some(key);
        if append {
            f.append_key = None;
            if !f.appends.contains(&key) {
                f.appends.push(key);
            }
            return;
        }
        // (a definition, its distance provisional until the node ends)
        let prov = Dist {
            n: f.d_in.n + 1,
            w: f.d_in.w + f.events + 1,
            p: f.d_in.p + if fclass == PAGE { f.events + 1 } else { 0 },
        };
        let after_page = if fclass == PAGE {
            PAGE_ORIGIN
        } else {
            f.after_page
        };
        let (kind, ap, via) = (f.kind, f.after_page, f.via);
        if self.debug == Some(key) {
            eprintln!("pure: written by {id} kind {kind} class {fclass} after_page {ap} via {via}");
        }
        let a = self.addr(key);
        a.hist[1] = a.hist[0];
        a.hist[0] = a.def;
        a.def = Def {
            prod: id,
            d: prov,
            page: fclass == PAGE,
            after_page,
            fold: NONE,
            fpos: 0,
            ver: 0,
        };
        if class == Class::List {
            a.app = None;
        }
        let first = a.writer != id;
        a.writer = id;
        if first {
            self.frames.last_mut().expect("a frame").writes.push(key);
            if let Some(c) = self.conds.last_mut() {
                c.written.push(key);
                if c.written.len() > 1 << 20 {
                    c.written.sort_unstable();
                    c.written.dedup();
                }
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn close(&mut self) {
        let Some(f) = self.frames.pop() else {
            return;
        };
        let page = f.class == PAGE;
        let cost = f.events + 1;
        let d = Dist {
            n: f.d_in.n + 1,
            w: f.d_in.w + cost,
            p: f.d_in.p + if page { cost } else { 0 },
        };
        let after_page = if page { PAGE_ORIGIN } else { f.after_page };
        self.total.n += 1;
        self.total.w += cost;
        if d.w > self.crit.w {
            self.crit_end = f.id;
        }
        self.crit = self.crit.max(d);
        if let Some(path) = self.path.as_mut() {
            let i = f.id as usize;
            if path.len() <= i {
                path.resize(i + 1, (NONE, 0));
            }
            path[i] = f.pred;
        }
        let k = self.kinds.entry(f.kind).or_default();
        k.nodes += 1;
        k.events += cost;
        k.reads += f.reads.len() as u64;
        k.writes += f.writes.len() as u64;
        self.ext_reads += f.reads.len() as u64;
        self.net_writes += f.writes.len() as u64;
        if f.unversioned {
            self.unversioned_nodes += 1;
        }
        if page {
            self.page_nodes += 1;
            self.page_events += cost;
        } else {
            self.body_nodes += 1;
            let mut direct = false;
            for r in &f.reads {
                if r.page {
                    direct = true;
                    *self.body_page_addrs.entry(r.key).or_default() += 1;
                }
            }
            if direct {
                self.body_page_direct += 1;
            }
            if f.after_page != 0 {
                self.body_page_trans += 1;
                // (by the page-chain address first read on the way, or,
                // for a page-chain node's own definition, how it came)
                let k = if f.after_page == PAGE_ORIGIN {
                    f.via
                } else {
                    f.after_page
                };
                *self.body_after_via.entry(k).or_default() += 1;
            }
        }
        // folding: join the open fold if of its class and a chain from its
        // last node (DESIGN 3.17, "Folding")
        let joined = self.fold_last != NONE
            && self.fold_class == f.class
            && (f.last_child == self.fold_last || f.reads.iter().any(|r| r.prod == self.fold_last));
        if !joined {
            self.fold_id = u32::try_from(self.fold_len.len()).unwrap_or(NONE);
            self.fold_len.push(0);
            self.fold_class = f.class;
        }
        let fold = self.fold_id;
        let fpos = self.fold_len[fold as usize];
        self.fold_len[fold as usize] += 1;
        self.fold_last = f.id;
        for r in &f.reads {
            if r.from_fold != NONE && r.from_fold != fold && !r.list {
                let len = self
                    .fold_len
                    .get(r.from_fold as usize)
                    .copied()
                    .unwrap_or(0);
                if r.from_fpos + 1 < len {
                    self.cuts
                        .insert((u64::from(r.from_fold) << 32) | u64::from(r.from_fpos));
                }
            }
        }
        for r in &f.reads {
            let a = self.addr(r.key);
            if a.reader_fold != fold {
                a.reader_fold = fold;
                self.fold_reads += 1;
            }
        }
        for &key in &f.writes {
            let a = self.addr(key);
            if a.def.prod == f.id {
                a.def.d = d;
                a.def.page = page;
                a.def.after_page = after_page;
                a.def.fold = fold;
                a.def.fpos = fpos;
                if a.writer_fold != fold {
                    a.writer_fold = fold;
                    self.fold_writes += 1;
                }
            }
        }
        for &key in &f.appends {
            self.appends += 1;
            let a = self.addr(key);
            let app = a.app.get_or_insert_with(Box::default);
            app.d = app.d.max(d);
            app.page |= page;
            if app.after_page == 0 {
                app.after_page = after_page;
            }
            app.last = Some(Def {
                prod: f.id,
                d,
                page,
                after_page,
                fold,
                fpos,
                ver: 0,
            });
        }
        self.list_reads += f.reads.iter().filter(|r| r.list).count() as u64;
        // the test of a conditional this node opened is decided
        for c in self.conds.iter_mut().rev() {
            if c.opener == f.id && c.test.is_none() {
                c.test = Some((d, after_page));
            }
        }
        // (a nested node's result is its parent's operand: the tokens an
        // expansion makes, a call's box; not `build_page`'s, whose result
        // is the page)
        let tokens = f.kind < K_BUILD_PAGE
            && f.kind != K_EXPAND + IF_TEST_CMD
            && f.kind != K_EXPAND + FI_OR_ELSE_CMD;
        if tokens && let Some(p) = self.frames.last_mut() {
            if d.w > p.d_in.w {
                p.pred = (f.id, NESTED_KEY);
            }
            p.d_in = p.d_in.max(d);
            if after_page != 0 && p.after_page == 0 {
                p.after_page = after_page;
                p.via = u64::MAX - 2;
            }
            p.last_child = f.id;
        }
        if let Some((nodes, lines)) = self.trace.as_mut() {
            let mut op = 0u64;
            for r in &f.reads {
                if f.fields.is_some() && r.list {
                    continue;
                }
                op = mix(op, mix(r.key, r.ver));
            }
            op = mix(op, f.fields.unwrap_or(f.arg));
            op = mix(op, f.toks);
            let flags = u8::from(f.unversioned);
            let mut rec = [0u8; 12];
            rec[0..2].copy_from_slice(&f.kind.to_le_bytes());
            rec[2] = f.class;
            rec[3] = flags;
            #[allow(clippy::cast_possible_truncation, reason = "a 32-bit hash")]
            rec[4..8].copy_from_slice(&(mix(f.src, 1) as u32).to_le_bytes());
            #[allow(clippy::cast_possible_truncation, reason = "a 32-bit hash")]
            rec[8..12].copy_from_slice(&(mix(op, 1) as u32).to_le_bytes());
            let _ = nodes.write_all(&rec);
            if f.kind == K_CALL + func_index(Func::LineBreak) {
                let n = u32::try_from(f.lines.len()).unwrap_or(0);
                let _ = lines.write_all(&n.to_le_bytes());
                for l in &f.lines {
                    let _ = lines.write_all(&l.to_le_bytes());
                }
            }
        }
    }

    /// Close the frames down to and including the innermost command.
    fn close_command(&mut self) {
        if !self.frames.last().is_some_and(|f| f.command) {
            if self.frames.is_empty() {
                return;
            }
            self.anomalies += 1;
        }
        while let Some(f) = self.frames.last() {
            let c = f.command;
            self.close();
            if c {
                break;
            }
        }
    }
}

fn func_index(f: Func) -> u16 {
    Func::ALL
        .iter()
        .position(|&g| g == f)
        .and_then(|i| u16::try_from(i).ok())
        .unwrap_or(99)
}

/// The tracer.
pub struct Stats {
    s: RefCell<State>,
}

impl Stats {
    pub fn new(trace: Option<&std::path::Path>) -> Stats {
        let mut s = State {
            vn: std::env::var("PARTEX_PURE_VN").is_ok_and(|v| v == "1"),
            debug: std::env::var("PARTEX_PURE_DEBUG")
                .ok()
                .and_then(|v| v.parse().ok()),
            spec_tail: std::env::var("PARTEX_PURE_SPEC_TAIL").is_ok_and(|v| v == "1"),
            path: std::env::var("PARTEX_PURE_PATH")
                .is_ok_and(|v| v == "1")
                .then(Vec::new),
            fold_last: NONE,
            fold_id: NONE,
            ..State::default()
        };
        if let Some(p) = trace {
            let open = |ext: &str| {
                std::fs::File::create(p.with_extension(ext))
                    .map(|f| std::io::BufWriter::with_capacity(1 << 20, f))
            };
            if let (Ok(a), Ok(b)) = (open("nodes"), open("lines")) {
                s.trace = Some((a, b));
            }
        }
        Stats { s: RefCell::new(s) }
    }
}

impl Tracker for Stats {
    const VALUES: bool = true;
    const SOFT_READS: bool = true;
    const CLASSES: bool = true;
    const PURE: bool = true;

    fn read(&self, cell: Cell) {
        // (eqtb's are read by `read_eqtb`, with their version; the names'
        // bookkeeping is structure; the others are read again as rows,
        // with versions, except where not: those are unversioned)
        let s = &mut *self.s.borrow_mut();
        match cell {
            Cell::Eqtb(_) => {}
            Cell::Hash(_) | Cell::HashNext(_) | Cell::Str(_) => {
                s.struct_reads += 1;
                s.top().events += 1;
            }
            // (read again as its row, `font_read`)
            Cell::Font(_) => {}
            Cell::FontTable | Cell::Read(_) | Cell::Out(_) | Cell::Random => {
                s.top().unversioned = true;
            }
        }
    }
    fn soft_read(&self, _cell: Cell, _level: i32) {}
    fn write(&self, cell: Cell) {
        let s = &mut *self.s.borrow_mut();
        match cell {
            Cell::Eqtb(p) => s.write(eqtb_key(p), Class::Normal),
            Cell::Hash(_) | Cell::HashNext(_) | Cell::Str(_) => {
                s.struct_writes += 1;
                s.top().events += 1;
            }
            _ => {}
        }
    }
    fn read_eqtb(&self, cell: Cell, content: impl Fn(bool) -> u128) {
        if let Cell::Eqtb(p) = cell {
            let v = fold128(content(false));
            self.s.borrow_mut().read(eqtb_key(p), Class::Normal, v);
        }
    }
    fn save_entry(&self, cell: Cell, _level: i32, at: Option<i32>) {
        if let (Cell::Eqtb(p), Some(at)) = (cell, at) {
            let s = &mut *self.s.borrow_mut();
            let d = s.addr(eqtb_key(p)).def;
            if s.debug == Some(eqtb_key(p)) {
                eprintln!("pure: save at {at}: def of {}", d.prod);
            }
            s.saved.insert(at, d);
        }
    }
    fn restore_entry(&self, cell: Cell, at: i32) {
        if let Cell::Eqtb(p) = cell {
            let s = &mut *self.s.borrow_mut();
            if s.debug == Some(eqtb_key(p)) {
                eprintln!(
                    "pure: restore at {at}: {:?}",
                    s.saved.get(&at).map(|d| d.prod)
                );
            }
            if let Some(d) = s.saved.remove(&at) {
                // (the group's end makes no definition: the one saved
                // reaches again)
                s.addr(eqtb_key(p)).def = d;
            }
        }
    }
    fn retract(&self, cell: Cell) {
        if let Cell::Eqtb(p) = cell {
            let s = &mut *self.s.borrow_mut();
            let key = eqtb_key(p);
            let Some(f) = s.frames.last_mut() else {
                return;
            };
            if f.reads.last().is_some_and(|r| r.key == key) {
                let r = f.reads.pop().expect("a read");
                s.addr(key).reader = r.prev_reader;
                // (its distance stays in the node's: an upper bound)
            }
        }
    }
    fn row_read(&self, row: Row, content: impl FnOnce() -> u128) {
        let (k, c) = row_key(row);
        let v = if c == Class::Struct {
            0
        } else {
            fold128(content())
        };
        self.s.borrow_mut().read(k, c, v);
    }
    fn value_read(&self, row: Row, version: impl FnOnce() -> u128) {
        let (k, c) = row_key(row);
        let v = if c == Class::Struct {
            0
        } else {
            fold128(version())
        };
        self.s.borrow_mut().read(k, c, v);
    }
    fn row_wrote(&self, row: Row, version: u128) {
        let (k, c) = row_key(row);
        let s = &mut *self.s.borrow_mut();
        s.write(k, c);
        if c == Class::Normal {
            let vn = s.vn;
            s.addr(k).known(fold128(version), vn);
        }
    }
    fn wrote_eqtb(&self, cell: Cell, content: impl FnOnce() -> u128) {
        if let Cell::Eqtb(p) = cell {
            let s = &mut *self.s.borrow_mut();
            let vn = s.vn;
            s.addr(eqtb_key(p)).known(fold128(content()), vn);
        }
    }
    fn value_wrote(&self, row: Row) {
        let (k, c) = row_key(row);
        self.s.borrow_mut().write(k, c);
    }
    fn call_begin(&self, f: Func, args: &[u128], _view: &dyn EngineView) -> Option<u32> {
        let s = &mut *self.s.borrow_mut();
        if f == Func::Output {
            // (open until the user's routine ends, across commands: its
            // reads are its caller's, the commands its own)
            s.calls.push(false);
            return None;
        }
        s.calls.push(true);
        if f == Func::ShipOut && s.setup.is_none() {
            s.setup = Some((s.total, s.crit));
        }
        let page = matches!(f, Func::PageStep | Func::ShipOut);
        s.open(K_CALL + func_index(f), u8::from(page), false);
        s.top().arg = args.first().map_or(0, |&a| fold128(a));
        None
    }
    fn call_end(&self, _view: &dyn EngineView) {
        let s = &mut *self.s.borrow_mut();
        if s.calls.pop() == Some(true) {
            if s.frames.last().is_some_and(|f| f.command) {
                s.anomalies += 1;
                return;
            }
            s.close();
        }
    }
    fn pure_begin(&self, output: bool) {
        let s = &mut *self.s.borrow_mut();
        s.close_command();
        s.open(K_PENDING, u8::from(output), true);
    }
    fn pure_cmd(&self, cmd: u16, _chr: i32, _mode: i32, output: bool) {
        let s = &mut *self.s.borrow_mut();
        let pending = s
            .frames
            .last()
            .is_some_and(|f| f.command && f.kind == K_PENDING);
        if !pending {
            s.close_command();
            s.open(K_PENDING, BODY, true);
        }
        let dep = s.cond_dep();
        if s.debug == Some(0) && !output && dep.is_some_and(|d| d.1 == PAGE_ORIGIN) {
            let c = s.conds.iter().rev().find(|c| c.test.is_some());
            let o = c.map_or(0, |c| c.opener);
            let id = s.top().id;
            eprintln!("pure: body command {cmd} node {id} under a page conditional opened by {o}");
        }
        let f = s.top();
        f.kind = cmd;
        let changed = output && f.class != PAGE;
        if changed {
            f.class = PAGE;
        }
        if let Some((d, a, via, o)) = dep {
            if d.w > f.d_in.w {
                f.pred = (o, COND_KEY);
            }
            f.d_in = f.d_in.max(d);
            if a != 0 && f.after_page == 0 {
                f.after_page = a;
                f.via = via;
            }
        }
        s.class_changes += u64::from(changed);
    }
    fn pure_expand(&self, begin: bool, cmd: u16, _chr: i32) {
        let s = &mut *self.s.borrow_mut();
        if begin {
            s.open(K_EXPAND + cmd, BODY, false);
        } else if s.frames.last().is_some_and(|f| !f.command) {
            s.close();
        } else {
            s.anomalies += 1;
        }
    }
    fn pure_macro(&self, _cs: i32) {
        self.s.borrow_mut().macros += 1;
    }
    fn pure_cond(&self, push: bool, depth: usize) {
        let s = &mut *self.s.borrow_mut();
        // (the engine's depth rules: a conditional it left another way,
        // with a file's end, ends here too)
        let keep = if push { depth.saturating_sub(1) } else { depth };
        if s.conds.len() > keep + usize::from(!push) {
            s.cond_mismatch += 1;
        }
        while s.conds.len() > keep {
            let Some(mut c) = s.conds.pop() else { break };
            c.written.sort_unstable();
            c.written.dedup();
            s.phis += c.written.len() as u64;
            let (td, tp) = c.test.unwrap_or_default();
            for &k in &c.written {
                let a = s.addr(k);
                // (a φ only where the definition reaching the `\fi` was
                // made in the arm: one a group's end put back is older)
                if a.def.prod == NONE || a.def.prod < c.opener {
                    continue;
                }
                a.def.d = a.def.d.max(td);
                if a.def.after_page == 0 {
                    a.def.after_page = tp;
                }
            }
            if let Some(p) = s.conds.last_mut() {
                p.written.extend_from_slice(&c.written);
            }
        }
        if push {
            s.conditionals += 1;
            let (opener, opener_kind) = (s.top().id, s.top().kind);
            s.conds.push(CondE {
                opener,
                opener_kind,
                test: None,
                written: Vec::new(),
            });
        }
    }
    fn pure_append(&self) {
        self.s.borrow_mut().pending_append = true;
    }
    fn pure_node(&self, begin: bool, kind: u8) {
        let s = &mut *self.s.borrow_mut();
        if begin {
            s.open(K_BUILD_PAGE + u16::from(kind), PAGE, false);
        } else if s.frames.last().is_some_and(|f| !f.command) {
            s.close();
        } else {
            s.anomalies += 1;
        }
    }
    fn pure_src(&self, cmd: u16, chr: i32, cs: i32) {
        let s = &mut *self.s.borrow_mut();
        let f = s.top();
        f.src = mix(
            f.src,
            mix(
                u64::from(cmd),
                u64::from(chr.cast_unsigned()) << 32 | u64::from(cs.cast_unsigned()),
            ),
        );
    }
    fn pure_level(&self, depth: usize) {
        let s = &mut *self.s.borrow_mut();
        let f = s.top();
        let l = (f.d_in, f.after_page, f.id);
        s.levels.truncate(depth);
        s.levels.resize(depth + 1, (Dist::default(), 0, NONE));
        s.levels[depth] = l;
    }
    fn pure_tok(&self, depth: usize, cmd: u16, chr: i32, cs: i32) {
        let s = &mut *self.s.borrow_mut();
        let l = s.levels.get(depth).copied();
        let f = s.top();
        if let Some((d, a, o)) = l {
            if d.w > f.d_in.w {
                f.pred = (o, LEVEL_KEY);
            }
            f.d_in = f.d_in.max(d);
            if a != 0 && f.after_page == 0 {
                f.after_page = a;
                f.via = u64::MAX - 1;
            }
        }
        f.toks = mix(
            f.toks,
            mix(
                u64::from(cmd),
                u64::from(chr.cast_unsigned()) << 32 | u64::from(cs.cast_unsigned()),
            ),
        );
    }
    fn pure_fields(&self, fields: u128) {
        let s = &mut *self.s.borrow_mut();
        s.top().fields = Some(fold128(fields));
    }
    fn pure_lines(&self, lines: &[u128]) {
        let s = &mut *self.s.borrow_mut();
        #[allow(clippy::cast_possible_truncation, reason = "a 32-bit hash")]
        let l = lines.iter().map(|&v| fold128(v) as u32).collect();
        s.top().lines = l;
    }
}

/// Write the report of `tex`'s build to `path`.
#[allow(clippy::too_many_lines)]
pub fn write<H: Host>(tex: &Tex<H, Stats>, path: &std::path::Path) -> std::io::Result<()> {
    let mut guard = tex.tracker().s.borrow_mut();
    let s = &mut *guard;
    while !s.frames.is_empty() {
        s.close();
    }
    if let Some((a, b)) = s.trace.as_mut() {
        a.flush()?;
        b.flush()?;
    }
    let folds = s.fold_len.len() as u64 + s.cuts.len() as u64;
    // projected bytes: a node 24 (kind, key, ranges) + 32 (its re-entry
    // handle: the input, nest, groups and conditionals as shared values),
    // 8 an operand, 8 a result, a φ 24 + 3·8; the definition index 16 a
    // definition
    let node_b = 56u64;
    let unfolded =
        s.total.n * node_b + s.ext_reads * 8 + s.net_writes * (8 + 16) + s.phis * (24 + 24);
    let folded = folds * node_b + s.fold_reads * 8 + s.fold_writes * (8 + 16) + s.phis * (24 + 24);
    let mut kinds: Vec<(u16, u64, u64, u64, u64)> = s
        .kinds
        .iter()
        .map(|(&k, v)| (k, v.nodes, v.events, v.reads, v.writes))
        .collect();
    kinds.sort_by_key(|k| std::cmp::Reverse(k.1));
    let mut out = String::new();
    let _ = writeln!(out, "{{");
    let _ = writeln!(
        out,
        "\"nodes\": {}, \"events\": {}, \"macro_calls\": {}, \"conditionals\": {}, \"phis\": {},",
        s.total.n, s.total.w, s.macros, s.conditionals, s.phis
    );
    let _ = writeln!(
        out,
        "\"ext_reads\": {}, \"net_writes\": {}, \"list_reads\": {}, \"appends\": {}, \"struct_reads\": {}, \"struct_writes\": {}, \"unversioned_nodes\": {}, \"anomalies\": {}, \"cond_mismatch\": {}, \"class_changes\": {},",
        s.ext_reads,
        s.net_writes,
        s.list_reads,
        s.appends,
        s.struct_reads,
        s.struct_writes,
        s.unversioned_nodes,
        s.anomalies,
        s.cond_mismatch,
        s.class_changes
    );
    let _ = writeln!(
        out,
        "\"critical_nodes\": {}, \"critical_events\": {}, \"page_path_events\": {},",
        s.crit.n, s.crit.w, s.crit.p
    );
    let (sw, sc) = s.setup.unwrap_or_default();
    let _ = writeln!(
        out,
        "\"setup_nodes\": {}, \"setup_events\": {}, \"setup_critical_nodes\": {}, \"setup_critical_events\": {},",
        sw.n, sw.w, sc.n, sc.w
    );
    let _ = writeln!(
        out,
        "\"page_nodes\": {}, \"page_events\": {}, \"body_nodes\": {}, \"body_reading_page\": {}, \"body_after_page\": {}, \"body_tail_reads\": {}, \"spec_tail\": {},",
        s.page_nodes,
        s.page_events,
        s.body_nodes,
        s.body_page_direct,
        s.body_page_trans,
        s.body_tail_reads,
        s.spec_tail
    );
    let _ = writeln!(
        out,
        "\"folds\": {}, \"fold_cuts\": {}, \"fold_reads\": {}, \"fold_writes\": {}, \"bytes_unfolded\": {}, \"bytes_folded\": {},",
        folds,
        s.cuts.len(),
        s.fold_reads,
        s.fold_writes,
        unfolded,
        folded
    );
    // the critical path's edges after the setup, by address
    let mut on_path: HashMap<u64, u64> = HashMap::new();
    let mut path_len = 0u64;
    if let Some(path) = s.path.as_ref() {
        let mut i = s.crit_end;
        // (a predecessor ended before its successor; ids are given at the
        // start, so a parent's nested predecessor has a larger one)
        let mut steps = 0;
        while let Some(&(p, k)) = path.get(i as usize) {
            if p == NONE || p == i || (p > i && k != NESTED_KEY) || steps > path.len() {
                break;
            }
            steps += 1;
            *on_path.entry(k).or_default() += 1;
            path_len += 1;
            i = p;
        }
    }
    let mut on_path: Vec<(u64, u64)> = on_path.into_iter().collect();
    on_path.sort_by_key(|&(k, n)| (std::cmp::Reverse(n), k));
    let mut via: Vec<(u64, u64)> = s.body_after_via.iter().map(|(&k, &n)| (k, n)).collect();
    via.sort_by_key(|&(k, n)| (std::cmp::Reverse(n), k));
    let mut addrs: Vec<(u64, u64)> = s.body_page_addrs.iter().map(|(&k, &n)| (k, n)).collect();
    addrs.sort_by_key(|&(k, n)| (std::cmp::Reverse(n), k));
    drop(guard);
    let name = |k: u64| {
        let n = match k {
            u64::MAX => "(a conditional's test)".to_string(),
            0xffff_ffff_ffff_fffe => "(a token list read)".to_string(),
            0xffff_ffff_ffff_fffd => "(a nested node)".to_string(),
            k if k >> 56 == 0 => tex.eqtb_loc_name(i32::try_from(k).unwrap_or(0)),
            k => key_name(k),
        };
        n.replace('\\', "\\\\").replace('"', "\\\"")
    };
    let _ = writeln!(out, "\"critical_path_walked\": {path_len},");
    for (label, list) in [
        ("body_page_addrs", &addrs),
        ("body_after_via", &via),
        ("critical_path_edges", &on_path),
    ] {
        let _ = writeln!(out, "\"{label}\": [");
        for (i, (k, n)) in list.iter().take(40).enumerate() {
            let comma = if i + 1 < list.len().min(40) { "," } else { "" };
            let _ = writeln!(out, "  [\"{}\", {n}, {k}]{comma}", name(*k));
        }
        let _ = writeln!(out, "],");
    }
    let _ = writeln!(out, "\"kinds\": [");
    for (i, (k, nodes, events, reads, writes)) in kinds.iter().enumerate() {
        let comma = if i + 1 < kinds.len() { "," } else { "" };
        let _ = writeln!(out, "  [{k}, {nodes}, {events}, {reads}, {writes}]{comma}");
    }
    let _ = writeln!(out, "]\n}}");
    std::fs::write(path, out)
}
