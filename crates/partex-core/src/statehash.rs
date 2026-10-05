//! A hash of the whole engine state, for early cutoff (`DESIGN.md`,
//! "Resume, early cutoff and output reuse").
//!
//! Two runs that reach equal states at a checkpoint produce the same rest
//! of the job, so the second run can stop and reuse the first one's
//! output. "Equal" must not depend on allocation: token lists, glue,
//! boxes and paragraph shapes are named by ids whose values depend on the
//! order things were allocated and freed, and glue carries a lineage
//! counter. The hash follows each id to its content and numbers ids in
//! the order it first meets them, so two states that differ only in their
//! ids hash alike.
//!
//! Completeness is what makes cutoff correct: [`Tex::state_hash`]
//! destructures the engine exhaustively, so a new field does not compile
//! until it is either hashed or listed as not part of the state. Where an
//! id could not be told from a plain value, the raw value is hashed: that
//! can only prevent a cutoff, never cause a wrong one.

use alloc::collections::BTreeMap;
use core::hash::{Hash, Hasher};

use partex_engine::stablehash::StableHasher;

use crate::cmds::{BOX_REF, GLUE_REF, SHAPE_REF};
use crate::host::Host;
use crate::input::InStateRecord;
use crate::mem::MemoryWord;
use crate::objs::{Glue, Obj, Shaped};
use crate::tex::Tex;
use crate::tok::Tokens;
use crate::track::Tracker;
use crate::web::{
    CALL, END_TEMPLATE, EQTB_SIZE, INT_BASE, LONG_CALL, LONG_OUTER_CALL, OUTER_CALL, TOKEN_LIST,
    UNDEFINED_CONTROL_SEQUENCE,
};
use crate::xregs::{Saved, ext_reg, is_word_kind};

/// Whether the file of a name is a cell of its own (see [`Tex::rest_hash_memo`]).
pub type Served<'a> = dyn Fn(&[u8]) -> bool + 'a;

/// Sub-hashes of shared state kept between [`Tex::state_hash_memo`]
/// calls (scratch: it keeps the chunks it answers for alive).
#[derive(Clone, Default)]
pub struct StateHashMemo(crate::hashmemo::HashMemo);

/// Whether the state hash takes each token list's hash as its store keeps
/// it (`TokStore::hash_of`), or computes it (the host's switch,
/// `PARTEX_LIST_HASH=0`; the same value either way).
pub static LIST_HASHES: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Whether a machine's `Rest` leaves out the line numbers of the input
/// (`line`, the line stack, and the lines read of every served file):
/// they are then a cell of their own (`MCell::Positions`, `machine.rs`),
/// whose numbers a rebuild can rename where lines moved (DESIGN.md §7.15).
/// The host's switch, `PARTEX_MACHINE_RENAME=1`; off by default.
pub static POSITION_CELLS: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Whether a machine's `Rest` leaves out the page builder's state (the
/// page, its totals and insertions, `last`, `page_disc`): it is then a
/// cell of its own (`MCell::Page`, `machine.rs`, DESIGN.md §7.16.2). The
/// host's switch, `PARTEX_MACHINE_PAGE_CELL=0` turns it off.
pub static PAGE_CELLS: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Whether a machine's `Rest` leaves out the values pdfTeX's `\pdflast…`
/// and `\pdfretval` read (`pdf::PdfLast`): they are then cells of their
/// own (`MCell::PdfLast`, `machine.rs`). The host's switch,
/// `PARTEX_MACHINE_PDF_LAST=0` turns it off.
pub static PDF_LAST_CELLS: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(true);

/// Hashes state, numbering the ids it meets.
struct Canon<'a> {
    h: StableHasher,
    /// The hashes of the sections finished so far (see `section`).
    sections: StableHasher,
    /// Token lists by the hashes their stores keep (unused: a list is
    /// hashed by its content, or combined by its version).
    #[allow(dead_code)]
    list_hashes: bool,
    /// Token lists by the versions their writes made (`TokStore::version`,
    /// a tracker with [`Tracker::VALUES`]): combined, never hashed.
    list_versions: bool,
    /// With `list_versions`: each list's version from its tokens (check
    /// mode's test of the lists' versions).
    list_by_tokens: bool,
    lineages: BTreeMap<u64, u32>,
    /// Sub-hashes of shared chunks, if a memo is at hand.
    memo: Option<&'a mut crate::hashmemo::HashMemo>,
    /// Whether a file being read is a cell of its own, by lines (a
    /// machine's `Line` cells): it then counts by the lines read, not by
    /// what is left of it.
    served: Option<&'a Served<'a>>,
    /// With `Tex::canon_strings`: the pool, whose strings made by the
    /// run are referred to by their characters.
    strs: Option<Strs<'a>>,
}

/// The string pool, to hash references by contents.
#[derive(Clone, Copy)]
struct Strs<'a> {
    pool: &'a crate::flat::Flat<u8>,
    start: &'a crate::flat::Flat<usize>,
    /// The first string the run made (`init_str_ptr`), and `str_ptr`.
    init: usize,
    ptr: usize,
}

impl Strs<'_> {
    /// String `n`'s key: its characters' hash if the run made it, else
    /// its number (the format's strings are every run's).
    fn key(&self, n: i32) -> (u8, u128) {
        match usize::try_from(n) {
            Ok(s) if s >= self.init && s < self.ptr => (
                1,
                StableHasher::of(
                    &*self
                        .pool
                        .range_all(self.start.get_all(s), self.start.get_all(s + 1)),
                ),
            ),
            _ => (0, u128::from(n.cast_unsigned())),
        }
    }
}

/// Seen before: its number; else a new number and `true`.
fn number<K: Ord + Copy>(m: &mut BTreeMap<K, u32>, k: K) -> (u32, bool) {
    let n = u32::try_from(m.len()).expect("ids");
    if let Some(&seen) = m.get(&k) {
        (seen, false)
    } else {
        m.insert(k, n);
        (n, true)
    }
}

impl Canon<'_> {
    fn new<'a>() -> Canon<'a> {
        Canon {
            h: StableHasher::new(),
            sections: StableHasher::new(),
            list_hashes: LIST_HASHES.load(core::sync::atomic::Ordering::Relaxed),
            list_versions: false,
            list_by_tokens: false,
            lineages: BTreeMap::new(),
            memo: None,
            served: None,
            strs: None,
        }
    }

    /// A string reference (by contents with `strs`).
    fn sref(&mut self, n: i32) {
        match self.strs {
            Some(s) => {
                let k = s.key(n);
                self.put(&k);
            }
            None => self.put(&n),
        }
    }

    /// The hash table: [`Canon::words`], but with `strs` each name made
    /// by the run is hashed by its characters.
    fn names(&mut self, t: &crate::journal::JVec<MemoryWord>) {
        let Some(strs) = self.strs else {
            self.words(t, 0);
            return;
        };
        let of = |ws: &[MemoryWord]| {
            let mut h = StableHasher::new();
            for (i, w) in ws.iter().enumerate() {
                if w.bits() != 0 {
                    (i, w.lh(), strs.key(w.rh())).hash(&mut h);
                }
            }
            h.finish128()
        };
        let n = t.chunks();
        self.put(&(t.len(), n, 1u8));
        for i in 0..n {
            let (ws, shared) = t.view(i);
            let h = match (shared, self.memo.as_deref_mut()) {
                (Some(a), Some(m)) if ws.len() == crate::journal::CHUNK => {
                    let p = m.pass;
                    m.names.get_or(p, a, |a| of(&a[..]))
                }
                _ => of(ws),
            };
            self.put(&h);
        }
    }

    /// End a section of the state: its own hash goes into the whole's
    /// (and to `parts`, which says where two states differ).
    fn section(
        &mut self,
        name: &'static str,
        parts: &mut Option<&mut alloc::vec::Vec<(&'static str, u128)>>,
    ) {
        let h = core::mem::replace(&mut self.h, StableHasher::new()).finish128();
        self.sections.write_u128(h);
        if let Some(p) = parts {
            p.push((name, h));
        }
    }

    fn put<T: Hash + ?Sized>(&mut self, v: &T) {
        v.hash(&mut self.h);
    }

    /// The words of a journaled table but those equal to `skip`, by
    /// chunk: each chunk's hash (by address when it is shared) with its
    /// number.
    fn words(&mut self, t: &crate::journal::JVec<MemoryWord>, skip: u64) {
        let of = |ws: &[MemoryWord]| {
            let mut h = StableHasher::new();
            for (i, w) in ws.iter().enumerate() {
                if w.bits() != skip {
                    (i, w.bits()).hash(&mut h);
                }
            }
            h.finish128()
        };
        let n = t.chunks();
        self.put(&(t.len(), n));
        for i in 0..n {
            let (ws, shared) = t.view(i);
            let h = match (shared, self.memo.as_deref_mut()) {
                (Some(a), Some(m)) if ws.len() == crate::journal::CHUNK => {
                    let p = m.pass;
                    m.words.get_or(p, a, |a| of(&a[..]))
                }
                _ => of(ws),
            };
            self.put(&h);
        }
    }

    /// The first `n` elements of a segmented vector, by segment (each
    /// segment's hash, by address when it is shared).
    fn segments<T: Hash + Copy + PartialEq + Default>(
        &mut self,
        v: &crate::flat::Flat<T>,
        n: usize,
        memo: impl Fn(&mut crate::hashmemo::HashMemo) -> Option<&mut crate::hashmemo::Memo<[T]>>,
    ) {
        let segs = v.segments(n);
        self.put(&(n.min(v.len_all()), segs.len()));
        for (s, shared) in segs {
            let h = match (shared, self.memo.as_deref_mut()) {
                (Some(a), Some(m)) => {
                    let p = m.pass;
                    match memo(m) {
                        Some(mm) => mm.get_or(p, a, StableHasher::of),
                        None => StableHasher::of(s),
                    }
                }
                _ => StableHasher::of(s),
            };
            self.put(&h);
        }
    }

    /// The hash of a shared piece (by address, with a memo).
    fn shared<T: Hash + ?Sized>(
        &mut self,
        a: &alloc::sync::Arc<T>,
        memo: impl Fn(&mut crate::hashmemo::HashMemo) -> &mut crate::hashmemo::Memo<T>,
    ) -> u128 {
        match self.memo.as_deref_mut() {
            Some(m) => {
                let p = m.pass;
                memo(m).get_or(p, a, StableHasher::of)
            }
            None => StableHasher::of(&**a),
        }
    }

    /// A token list (an entry's value, a level's list).
    fn tok(&mut self, t: Option<&Tokens>) {
        let Some(t) = t else {
            self.put(&0u8);
            return;
        };
        if self.list_versions {
            let v = if self.list_by_tokens {
                t.version_by_tokens()
            } else {
                t.version()
            };
            self.put(&(2u8, v));
            return;
        }
        // (by contents, not identity: whether two references share a
        // list TeX never observes)
        let h = StableHasher::of(&(t.protected(), t.tokens()));
        self.put(&(1u8, h));
    }

    fn lineage(&mut self, l: u64) {
        let (n, _) = number(&mut self.lineages, l);
        self.put(&n);
    }

    fn glue(&mut self, g: Option<&Glue>) {
        let g = g.copied().unwrap_or(Glue::ZERO);
        self.put(&g.spec);
        self.lineage(g.lineage);
    }

    fn boxed(&mut self, b: Option<&alloc::sync::Arc<partex_engine::node::BoxNode>>) {
        if self.list_versions {
            // (a box is a value carrying its version: combined; check
            // mode's test makes it from the box's parts instead)
            let v = b.map_or(0, |b| {
                if self.list_by_tokens {
                    b.parts_version()
                } else {
                    b.ver
                }
            });
            self.put(&(3u8, v));
            return;
        }
        self.put(&b.map(|b| &**b));
    }

    fn shape(&mut self, sh: Option<&alloc::sync::Arc<Shaped>>) {
        self.put(&sh.map(|s| &**s));
    }

    /// An eqtb word (or a saved copy of one) of location `loc`, with the
    /// value it holds beside it.
    fn word(&mut self, loc: i32, w: MemoryWord, o: Option<&Obj>) {
        if (INT_BASE..=EQTB_SIZE).contains(&loc) {
            self.put(&w.bits()); // an integer or dimension
            return;
        }
        let (t, e) = (w.b0(), w.rh());
        self.put(&(t, w.b1()));
        match t {
            CALL | LONG_CALL | OUTER_CALL | LONG_OUTER_CALL | END_TEMPLATE => {
                self.put(&e);
                self.tok(o.and_then(Obj::toks));
            }
            GLUE_REF => self.glue(match o {
                Some(Obj::Glue(g)) => Some(g),
                _ => None,
            }),
            SHAPE_REF => self.shape(match o {
                Some(Obj::Shape(sh)) => Some(sh),
                _ => None,
            }),
            BOX_REF => self.boxed(match o {
                Some(Obj::Box(b)) => Some(b),
                _ => None,
            }),
            _ => self.put(&e),
        }
    }

    /// A register above 255 (its kind decides what the word holds).
    fn ext_word(&mut self, loc: i32, w: MemoryWord, o: Option<&Obj>) {
        self.put(&loc);
        if is_word_kind(ext_reg(loc).0) {
            self.put(&w.bits());
        } else {
            self.word(-1, w, o);
        }
    }

    fn saved(&mut self, s: &Saved) {
        self.put(&s.level);
        self.ext_word(s.loc, s.word, s.obj.as_ref());
    }

    fn input(&mut self, r: &InStateRecord) {
        let InStateRecord {
            ref list,
            state,
            index,
            start,
            loc,
            limit,
            name,
        } = *r;
        self.put(&(state, index, limit));
        if state == TOKEN_LIST {
            // (a token list's `name` is the control sequence a macro was
            // called by, §390: a location, not a string)
            self.put(&name);
        } else {
            self.sref(name);
        }
        if state == TOKEN_LIST {
            self.tok(list.as_ref());
            self.put(&loc); // (an index into the list)
        } else {
            self.put(&(start, loc)); // (buffer positions)
        }
    }
}

/// What a state hash covers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// Everything.
    All,
    /// All but the tracked cells (eqtb, the hash, registers above 255,
    /// fonts, streams).
    Untracked,
    /// All but eqtb's words.
    Rest,
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// The hash of everything the rest of the job depends on (see the
    /// module documentation). Files being read contribute how much of
    /// them is left, not what: the caller checks that the rest of each
    /// file is the same.
    ///
    /// # Panics
    ///
    /// Only if the engine's tables are inconsistent.
    #[must_use]
    pub fn state_hash(&self) -> u128 {
        self.state_hash_into(None)
    }

    /// [`Tex::state_hash`], with the sub-hashes of shared chunks looked up
    /// in `memo` (`hashmemo.rs`): states that share most chunks, such as
    /// a session's checkpoints and the job running on from one, cost what
    /// they do not share. The same value as without it.
    pub fn state_hash_memo(&self, memo: &mut StateHashMemo) -> u128 {
        let m = &mut memo.0;
        m.begin();
        let h = self.state_hash_timed(None, Scope::All, Some((m, &|_| false)));
        m.end();
        h
    }

    /// The hashes of the parts of the state [`Tex::state_hash`] covers,
    /// by name (for `PARTEX_WATCH_DEBUG`: where two states differ; ids
    /// are numbered by first use, so a difference can show again in
    /// later parts).
    #[must_use]
    pub fn state_hash_parts(&self) -> alloc::vec::Vec<(&'static str, u128)> {
        let mut parts = alloc::vec::Vec::new();
        self.state_hash_into(Some(&mut parts));
        parts
    }

    /// A cheap part of [`Tex::state_hash`]: the input stack, the buffer
    /// and how much of each file is left. Two states whose full hashes
    /// are equal have equal input hashes (it is canonical over that part
    /// alone), so a mismatch here saves computing the full hash.
    #[must_use]
    pub fn input_hash(&self) -> u128 {
        let mut c = Canon::new();
        for r in &self.input_stack[..self.input_ptr] {
            c.input(r);
        }
        c.input(&self.cur_input);
        c.put(&(
            self.input_ptr,
            self.in_open,
            self.open_parens,
            self.base_ptr,
        ));
        c.put(&self.buffer.prefix(self.first.max(self.last)));
        c.put(&(self.first, self.last, self.line));
        let left = |f: &Option<crate::input::AlphaFile>| f.as_ref().map(|f| f.data.len() - f.pos);
        c.put(
            &self
                .input_file
                .iter()
                .map(left)
                .collect::<alloc::vec::Vec<_>>(),
        );
        c.put(
            &self
                .read_file
                .iter()
                .map(left)
                .collect::<alloc::vec::Vec<_>>(),
        );
        c.h.finish128()
    }

    /// [`Tex::state_hash`] and [`Tex::state_hash_parts`] in one pass.
    #[must_use]
    pub fn state_hash_and_parts(&self) -> (u128, alloc::vec::Vec<(&'static str, u128)>) {
        let mut parts = alloc::vec::Vec::new();
        let h = self.state_hash_into(Some(&mut parts));
        (h, parts)
    }

    /// The input files' slots and stacks, readable (for debugging the
    /// machine).
    #[must_use]
    pub fn input_files_debug(&self) -> alloc::string::String {
        let files: alloc::vec::Vec<_> = self
            .input_file
            .iter()
            .map(|f| {
                f.as_ref().map(|f| {
                    (
                        alloc::string::String::from_utf8_lossy(&f.name).into_owned(),
                        f.lines,
                        f.data.len() - f.pos,
                    )
                })
            })
            .collect();
        alloc::format!(
            "files {files:?} lines {:?} grp {:?} if {:?} eof {:?} pseudo {}",
            self.line_stack,
            self.grp_stack,
            self.if_stack,
            self.eof_seen,
            self.pseudo_files.len()
        )
    }

    /// The counters that number things (strings, control sequences, PDF
    /// objects, names) and where the tables of `other` first differ from
    /// this one's (for debugging).
    #[must_use]
    pub fn numbering_difference(&self, other: &Self) -> alloc::string::String {
        let first_str = (256..self.str_ptr.min(other.str_ptr))
            .find(|&n| self.str_bytes(n) != other.str_bytes(n));
        let show = |t: &Self, n: Option<usize>| {
            n.map(|n| alloc::string::String::from_utf8_lossy(t.str_bytes(n)).into_owned())
        };
        let (a, b) = (&self.pdf.objs, &other.pdf.objs);
        let first_obj = (1..a.tab.len().min(b.tab.len())).find(|&k| a.tab[k] != b.tab[k]);
        let first_dest = a
            .dest_names
            .iter()
            .zip(b.dest_names.iter())
            .position(|(x, y)| x != y);
        // (every difference, counted, the first few shown)
        let strs: alloc::vec::Vec<usize> = (256..self.str_ptr.min(other.str_ptr))
            .filter(|&n| self.str_bytes(n) != other.str_bytes(n))
            .collect();
        let objs: alloc::vec::Vec<usize> = (1..a.tab.len().min(b.tab.len()))
            .filter(|&k| a.tab[k] != b.tab[k])
            .collect();
        let dests = a
            .dest_names
            .iter()
            .zip(b.dest_names.iter())
            .filter(|(x, y)| x != y)
            .count();
        let trees: usize = a
            .trees
            .iter()
            .zip(b.trees.iter())
            .map(|(x, y)| x.iter().filter(|(k, v)| y.get(*k) != Some(*v)).count())
            .sum();
        let hash_text: usize = (0..self.hash.len().min(other.hash.len()))
            .filter(|&p| self.hash[p] != other.hash[p])
            .count();
        let mut detail = alloc::format!(
            "; all: {} strings {:?}, {} objects {:?}, {} dests, {} tree entries, {} hash words",
            strs.len(),
            &strs[..strs.len().min(8)],
            objs.len(),
            &objs[..objs.len().min(8)],
            dests,
            trees,
            hash_text
        );
        for &k in objs.iter().take(4) {
            use core::fmt::Write;
            let _ = write!(
                detail,
                "\n    obj {k}: {:?}\n     vs {:?}",
                a.tab[k], b.tab[k]
            );
        }
        alloc::format!(
            "str_ptr {} vs {} (first differs {:?}: {:?} vs {:?}); cs_count {} vs {}; hash_used {} vs {}; obj_ptr {} vs {} (first differs {:?}); dests {} vs {} (first differs {:?}){detail}",
            self.str_ptr,
            other.str_ptr,
            first_str,
            show(self, first_str),
            show(other, first_str),
            self.cs_count,
            other.cs_count,
            self.hash_used,
            other.hash_used,
            a.obj_ptr,
            b.obj_ptr,
            first_obj.map(|k| (
                k,
                alloc::format!("{:?}", a.tab[k]),
                alloc::format!("{:?}", b.tab[k])
            )),
            a.dest_names.len(),
            b.dest_names.len(),
            first_dest.map(|k| (a.dest_names[k].clone(), b.dest_names[k].clone())),
        )
    }

    /// The scanner's and main control's scalars that differ from
    /// `other`'s, with both values (for debugging the machine).
    #[must_use]
    pub fn scalars_difference(&self, other: &Self) -> alloc::string::String {
        let mut out = alloc::vec::Vec::new();
        macro_rules! cmp {
            ($($f:ident),*) => {$(
                if self.$f != other.$f {
                    out.push(alloc::format!("{} {:?}/{:?}", stringify!($f), self.$f, other.$f));
                }
            )*};
        }
        cmp!(
            cur_val,
            cur_glue,
            glue_origin,
            cur_cmd,
            cur_chr,
            cur_cs,
            cur_tok,
            cur_val_level,
            radix,
            cur_order,
            last_badness,
            expand_depth_count
        );
        out.join(", ")
    }

    /// Where the engine's copies of line numbers differ from `other`'s
    /// (for debugging the machine): the nest's `mode_line` (§213), the
    /// conditions' lines (§489), `skip_line` (§493), `pack_begin_line`
    /// (§661), and `line` itself.
    #[must_use]
    pub fn line_copies_difference(&self, other: &Self) -> alloc::string::String {
        let mut out = alloc::vec::Vec::new();
        let ml = |t: &Self| {
            t.nest
                .iter()
                .chain(core::iter::once(&t.cur_list))
                .map(|l| l.ml)
                .collect::<alloc::vec::Vec<_>>()
        };
        if ml(self) != ml(other) {
            out.push(alloc::format!("mode_line {:?}/{:?}", ml(self), ml(other)));
        }
        let ifs = |t: &Self| {
            let mut v: alloc::vec::Vec<i32> = t.cond_stack.iter().map(|c| c.line).collect();
            v.push(t.if_line);
            v
        };
        if ifs(self) != ifs(other) {
            out.push(alloc::format!("if_line {:?}/{:?}", ifs(self), ifs(other)));
        }
        if self.skip_line != other.skip_line {
            out.push(alloc::format!(
                "skip_line {}/{}",
                self.skip_line,
                other.skip_line
            ));
        }
        if self.pack_begin_line != other.pack_begin_line {
            out.push(alloc::format!(
                "pack_begin_line {}/{}",
                self.pack_begin_line,
                other.pack_begin_line
            ));
        }
        if self.line != other.line {
            out.push(alloc::format!("line {}/{}", self.line, other.line));
        }
        out.join(", ")
    }

    /// The parts of `Rest` as a machine hashes it, with the files `served`
    /// counted by lines (for debugging the machine).
    #[must_use]
    pub fn rest_hash_parts_served(
        &self,
        served: &dyn Fn(&[u8]) -> bool,
    ) -> alloc::vec::Vec<(&'static str, u128)> {
        let mut parts = alloc::vec::Vec::new();
        let mut memo = crate::hashmemo::HashMemo::default();
        self.state_hash_timed(Some(&mut parts), Scope::Rest, Some((&mut memo, served)));
        parts
    }

    /// Where the save stack differs from `other`'s (for debugging the
    /// machine): its pointers, and the first entries that differ, with
    /// the location an eqtb entry saves.
    #[must_use]
    pub fn save_stack_difference(&self, other: &Self) -> alloc::string::String {
        let (a, b) = (self.save_ptr, other.save_ptr);
        let mut out = alloc::vec![alloc::format!(
            "save_ptr {a}/{b} level {}/{} group {}/{} boundary {}/{}",
            self.cur_level,
            other.cur_level,
            self.cur_group,
            other.cur_group,
            self.cur_boundary,
            other.cur_boundary
        )];
        let top = usize::try_from(a.min(b)).unwrap_or(0);
        let mut n = 0;
        for p in 0..top {
            let (x, y) = (self.save_stack[p], other.save_stack[p]);
            let (ex, ey) = (
                self.save_eqtb.get(p).copied().unwrap_or(false),
                other.save_eqtb.get(p).copied().unwrap_or(false),
            );
            if x.bits() != y.bits() || ex != ey {
                n += 1;
                if n <= 6 {
                    let what = if ex {
                        self.eqtb_loc_name(self.save_stack[p + 1].rh())
                    } else {
                        alloc::format!("word {:x}/{:x}", x.bits(), y.bits())
                    };
                    out.push(alloc::format!("entry {p}: {what}"));
                }
            }
        }
        out.push(alloc::format!("{n} entries differ"));
        out.join("; ")
    }

    /// The page builder's list hashed with the contents of the hboxes on
    /// it left out (their glue setting and list): what differs between
    /// two pages that differ only inside their lines (for debugging).
    #[must_use]
    pub fn page_skeleton_hash(&self) -> u128 {
        use core::hash::Hash;
        let mut h = StableHasher::new();
        for n in self.page.list.iter() {
            match n {
                partex_engine::node::Node::Box(b) if !b.vertical => {
                    (b.width, b.height, b.depth, b.shift, b.subtype).hash(&mut h);
                }
                n => n.hash(&mut h),
            }
        }
        (&self.page.contents, &self.page.so_far, &self.page.ins).hash(&mut h);
        h.finish128()
    }

    /// Where the PDF writer's output state differs from `other`'s (for
    /// debugging).
    #[must_use]
    pub fn pdf_out_difference(&self, other: &Self) -> alloc::string::String {
        self.pdf.out.difference(&other.pdf.out)
    }

    /// The PDF writer's state by part (for debugging).
    #[must_use]
    pub fn pdf_hash_parts(&self) -> alloc::vec::Vec<(&'static str, u128)> {
        self.pdf.hash_parts()
    }

    /// The parts of `Rest` in which `other` differs from this state, by
    /// field where the parts' own comparisons tell (for the machine's
    /// audit, `PARTEX_MACHINE_AUDIT`): `rest.<section>` for a section of
    /// [`Tex::rest_hash_parts`]; `scalars.<field>` inside the scanner's
    /// result; `pdf.<field>`, `pdf.ship.<field>`, `pdf.out.<field>` and
    /// `pdf.objs.head[<type>]` inside the PDF writer. A field is named if
    /// it differs and its part's hash does (a part may compare a field
    /// its hash leaves out).
    #[must_use]
    pub fn rest_field_differences(&self, other: &Self) -> alloc::vec::Vec<alloc::string::String> {
        use alloc::string::ToString;
        let mut out = alloc::vec::Vec::new();
        let first_word =
            |f: &alloc::string::String| f.split_whitespace().next().unwrap_or_default().to_string();
        let last_cells = PDF_LAST_CELLS.load(core::sync::atomic::Ordering::Relaxed);
        for ((name, x), (_, y)) in self.rest_hash_parts().iter().zip(other.rest_hash_parts()) {
            if *x == y {
                continue;
            }
            match *name {
                "pdf" => {
                    for ((part, a), (_, b)) in
                        self.pdf_hash_parts().iter().zip(other.pdf_hash_parts())
                    {
                        // (with the `\pdflast…` values as cells, `Rest`
                        // leaves them out, `WithoutLast`)
                        let last =
                            part.starts_with("last_") && *part != "last_match" || *part == "retval";
                        if *a == b || (last && last_cells) {
                            continue;
                        }
                        let before = out.len();
                        match *part {
                            "ship" => out.extend(
                                self.pdf
                                    .ship
                                    .differences(&other.pdf.ship)
                                    .iter()
                                    // (the fonts hash nothing: they are
                                    // cells, and their glyphs `Glyphs`)
                                    .filter(|f| !f.starts_with("fonts"))
                                    .map(|f| alloc::format!("pdf.ship.{}", first_word(f))),
                            ),
                            "out" => out.extend(
                                self.pdf
                                    .out
                                    .differences(&other.pdf.out)
                                    .iter()
                                    .map(|f| alloc::format!("pdf.out.{}", first_word(f))),
                            ),
                            "objs" => out.extend(
                                (0..self.pdf.objs.head.len())
                                    .filter(|&t| self.pdf.objs.head[t] != other.pdf.objs.head[t])
                                    .map(|t| alloc::format!("pdf.objs.head[{t}]")),
                            ),
                            _ => {}
                        }
                        if out.len() == before {
                            out.push(alloc::format!("pdf.{part}"));
                        }
                    }
                }
                "scalars: cur_val" => {
                    let before = out.len();
                    if self.cur_val != other.cur_val {
                        out.push("scalars.cur_val".to_string());
                    }
                    if self.cur_glue != other.cur_glue || self.glue_origin != other.glue_origin {
                        out.push("scalars.cur_glue".to_string());
                    }
                    if out.len() == before {
                        out.push(alloc::format!("rest.{name}"));
                    }
                }
                name => out.push(alloc::format!("rest.{name}")),
            }
        }
        out
    }

    /// The PDF object table's size, the first entry that differs from
    /// `other`'s, and each side's entry there (for `PARTEX_WATCH_DEBUG`).
    #[must_use]
    pub fn pdf_objs_difference(&self, other: &Self) -> alloc::string::String {
        let (a, b) = (&self.pdf.objs.tab, &other.pdf.objs.tab);
        let first = a.iter().zip(b.iter()).position(|(x, y)| {
            let h = |e: &crate::pdf::objtab::Entry| {
                let mut s = StableHasher::new();
                e.hash(&mut s);
                s.finish128()
            };
            h(x) != h(y)
        });
        let at = first.map(|i| alloc::format!("{i}: {:?} / {:?}", a[i], b[i]));
        // (the heads of the lists of objects by type, `head_tab`)
        let heads: alloc::vec::Vec<usize> = (0..self.pdf.objs.head.len())
            .filter(|&t| self.pdf.objs.head[t] != other.pdf.objs.head[t])
            .collect();
        alloc::format!(
            "{} objects vs {} (obj_ptr {} vs {}), first difference {}; heads by type {heads:?}; ship: {}; out: {}",
            a.len(),
            b.len(),
            self.pdf.objs.obj_ptr,
            other.pdf.objs.obj_ptr,
            at.unwrap_or_default(),
            self.pdf.ship.differences(&other.pdf.ship).join(", "),
            self.pdf.out.differences(&other.pdf.out).join(", ")
        )
    }

    /// The eqtb locations in use with a hash of each, its token lists,
    /// glue and boxes by content (for `PARTEX_WATCH_DEBUG`).
    ///
    /// # Panics
    ///
    /// Only if the engine's tables are inconsistent.
    #[must_use]
    pub fn eqtb_cell_hashes(&self) -> alloc::vec::Vec<(i32, u128)> {
        let undefined =
            self.eqtb[usize::try_from(UNDEFINED_CONTROL_SEQUENCE).expect("eqtb")].bits();
        (0..)
            .zip(self.eqtb.slices().flatten())
            .filter(|&(l, w)| (INT_BASE..=EQTB_SIZE).contains(&l) || w.bits() != undefined)
            .map(|(l, &w)| {
                let mut c = Canon::new();
                c.word(l, w, self.peek_obj(l));
                (l, c.h.finish128())
            })
            .collect()
    }

    /// The hash of one eqtb location, as [`Tex::eqtb_cell_hashes`] gives
    /// it (by content: a macro by its tokens), for a location in eqtb.
    #[must_use]
    pub fn eqtb_cell_hash(&self, loc: i32) -> Option<u128> {
        let i = usize::try_from(loc).ok().filter(|&i| i < self.eqtb.len())?;
        let w = self.eqtb[i];
        let mut c = Canon::new();
        c.word(loc, w, self.peek_obj(loc));
        Some(c.h.finish128())
    }

    /// The content of a tracked cell, hashed so that two engines whose ids
    /// differ compare alike (a macro by its tokens, glue by its spec; the
    /// level of an integer or dimension included). For the sanitizer
    /// (`sanitize.rs`).
    #[must_use]
    pub fn cell_content(&self, cell: crate::track::Cell) -> u128 {
        use crate::track::Cell;
        let mut c = Canon::new();
        match cell {
            Cell::Hash(p) | Cell::HashNext(p) => {
                let w = usize::try_from(p - crate::web::HASH_BASE)
                    .ok()
                    .filter(|&i| i < self.hash.len())
                    .map_or(0, |i| self.hash[i].bits());
                let w = crate::mem::MemoryWord::from_bits(w);
                c.put(&if matches!(cell, Cell::HashNext(_)) {
                    w.lh()
                } else {
                    w.rh()
                });
            }
            Cell::Eqtb(p) => return self.eqtb_content(p, self.peek_eqtb(p)),
            other => c.put(&self.family_digest(other)),
        }
        c.h.finish128()
    }

    /// [`Tex::cell_content`] of eqtb location `p` if it held word `w`
    /// (its level and the objects `w` names as they are now).
    #[must_use]
    pub(crate) fn eqtb_content(&self, p: i32, w: MemoryWord) -> u128 {
        self.eqtb_content_with(p, w, false)
    }

    /// [`Tex::cell_content`] of eqtb location `p` with the versions of the
    /// lists it names made from their tokens: check mode's test that both
    /// the entry's version and its lists' were made at their writes.
    #[must_use]
    pub(crate) fn eqtb_content_by_tokens(&self, p: i32) -> u128 {
        self.eqtb_content_with(p, self.peek_eqtb(p), true)
    }

    fn eqtb_content_with(&self, p: i32, w: MemoryWord, by_tokens: bool) -> u128 {
        use crate::web::{ETEX_PEN_BASE, OUTPUT_ROUTINE_LOC, TOK_VAL};
        use crate::xregs::EXT_BASE;
        let mut c = Canon::new();
        // (a list the entry names is a value with its version: combined)
        c.list_versions = T::VALUES;
        c.list_by_tokens = by_tokens;
        {
            {
                let toks = (OUTPUT_ROUTINE_LOC..ETEX_PEN_BASE).contains(&p)
                    || (p >= EXT_BASE && ext_reg(p).0 == TOK_VAL);
                let o = self.peek_obj(p);
                if toks {
                    c.put(&(w.b0(), w.b1()));
                    c.tok(o.and_then(Obj::toks));
                } else if p >= EXT_BASE {
                    c.ext_word(p, w, o);
                } else {
                    c.word(p, w, o);
                }
                if p >= INT_BASE
                    && (p <= EQTB_SIZE || (p >= EXT_BASE && is_word_kind(ext_reg(p).0)))
                {
                    c.put(&self.peek_xeq_level(p));
                }
            }
        }
        c.h.finish128()
    }

    /// The locations of the registers above 255 that differ from their
    /// default.
    #[must_use]
    pub fn xreg_locs(&self) -> alloc::vec::Vec<i32> {
        self.xregs
            .cells()
            .filter(|&(l, w, v)| {
                let (dw, dv) = crate::xregs::ExtRegs::default_cell(l);
                w.bits() != dw.bits() || v != dv
            })
            .map(|(l, _, _)| l)
            .collect()
    }

    /// How eqtb cells differing from `other`'s differ: by the name at the
    /// location, or with the same name (for the convergence probe).
    #[must_use]
    pub fn eqtb_diff_detail(
        &self,
        other: &Self,
        cells: &[crate::track::Cell],
    ) -> alloc::string::String {
        let (mut renamed, mut same) = (alloc::vec::Vec::new(), alloc::vec::Vec::new());
        for c in cells {
            if let crate::track::Cell::Eqtb(p) = *c {
                let (a, b) = (self.eqtb_loc_name(p), other.eqtb_loc_name(p));
                if a == b {
                    same.push(a);
                } else {
                    renamed.push(alloc::format!("{a}/{b}"));
                }
            }
        }
        let names = |t: &Self| {
            let mut s = alloc::collections::BTreeSet::new();
            let (pool, start) = (t.str_pool.to_vec(), t.str_start.to_vec());
            for i in 0..t.hash.len() {
                if let Ok(x) = usize::try_from(t.hash[i].rh())
                    && x > 0
                    && x < t.str_ptr
                {
                    s.insert(
                        alloc::string::String::from_utf8_lossy(&pool[start[x]..start[x + 1]])
                            .into_owned(),
                    );
                }
            }
            s
        };
        let (x, y) = (names(self), names(other));
        let only_old: alloc::vec::Vec<_> = x.difference(&y).take(10).collect();
        let only_new: alloc::vec::Vec<_> = y.difference(&x).take(10).collect();
        alloc::format!(
            "only old {only_old:?} only new {only_new:?} hash_used {} {} cs {} {} str {} {} renamed {} {:?} same {} {:?}",
            self.hash_used,
            other.hash_used,
            self.cs_count,
            other.cs_count,
            self.str_ptr,
            other.str_ptr,
            renamed.len(),
            &renamed[..renamed.len().min(12)],
            same.len(),
            &same[..same.len().min(40)]
        )
    }

    /// Each input stack record with what its token list hashes to (for
    /// the sanitizer's diagnostics).
    #[must_use]
    pub fn input_stack_debug(&self) -> alloc::vec::Vec<alloc::string::String> {
        let mut out = alloc::vec::Vec::new();
        let recs: alloc::vec::Vec<InStateRecord> = self.input_stack[..self.input_ptr]
            .iter()
            .cloned()
            .chain(core::iter::once(self.cur_input.clone()))
            .collect();
        for r in recs {
            let list = if r.state == TOKEN_LIST {
                if let Some(t) = &r.list {
                    alloc::format!(
                        "list len {} hash {:x} {:?}",
                        t.len(),
                        StableHasher::of(&(t.protected(), t.tokens())) % (1 << 32),
                        &t[..t.len().min(6)]
                    )
                } else {
                    alloc::string::String::from("list (none)")
                }
            } else {
                alloc::string::String::from("file")
            };
            let key = |n: i32| match usize::try_from(n) {
                Ok(s) if s >= self.init_str_ptr && s < self.str_ptr => alloc::format!(
                    "{:?}",
                    alloc::string::String::from_utf8_lossy(
                        &self
                            .str_pool
                            .range_all(self.str_start.get_all(s), self.str_start.get_all(s + 1))
                    )
                ),
                _ => alloc::format!("#{n}"),
            };
            out.push(alloc::format!(
                "state {} index {} start {} loc {} limit {} name {} {list}",
                r.state,
                r.index,
                r.start,
                r.loc,
                r.limit,
                key(r.name)
            ));
        }
        out.push(alloc::format!(
            "input_ptr {} in_open {} open_parens {} base_ptr {} init_str {}",
            self.input_ptr,
            self.in_open,
            self.open_parens,
            self.base_ptr,
            self.init_str_ptr
        ));
        out
    }

    /// How many font slots the arrays reach, how many fonts are in the
    /// order of loading, and `font_ptr` (for the sanitizer's diagnostics).
    #[must_use]
    pub fn font_slot_count(&self) -> usize {
        self.fonts.metrics.len()
    }
    #[must_use]
    pub fn font_order_len(&self) -> usize {
        self.fonts.order.len()
    }
    #[must_use]
    pub fn font_count(&self) -> i32 {
        self.font_ptr
    }

    /// The font slots whose versions differ from `other`'s, and in what
    /// (for the convergence probe).
    #[must_use]
    pub fn font_slot_differences(&self, other: &Self) -> alloc::string::String {
        use core::hash::Hash;
        let n = self.fonts.metrics.len().max(other.fonts.metrics.len());
        let mut out = alloc::vec::Vec::new();
        for f in 0..i32::try_from(n).unwrap_or(0) {
            if self.font_version(f) == other.font_version(f) {
                continue;
            }
            let i = usize::try_from(f).unwrap_or(0);
            let part = |t: &Self| -> alloc::vec::Vec<u128> {
                let fs = &t.fonts;
                if i >= fs.metrics.len() {
                    return alloc::vec::Vec::new();
                }
                let h = |x: &dyn Fn(&mut StableHasher)| {
                    let mut s = StableHasher::new();
                    x(&mut s);
                    s.finish128()
                };
                alloc::vec![
                    h(&|s| fs.rank.get(i).is_some_and(|&r| r > 0).hash(s)),
                    h(&|s| (fs.ident[i], fs.retagged[i]).hash(s)),
                    h(&|s| fs.metrics[i].params.hash(s)),
                    h(&|s| (fs.hyphen_char[i], fs.skew_char[i]).hash(s)),
                    h(&|s| fs.codes[i].hash(s)),
                    h(&|s| fs.expand[i].hash(s)),
                    h(&|s| t.string_bytes(fs.name[i]).hash(s)),
                    h(&|s| t.pdf.ship.fonts.get(i).map(|p| p.used).hash(s)),
                    h(&|s| t.pdf.ship.fonts.get(i).map(|p| p.num).hash(s)),
                    h(&|s| t
                        .pdf
                        .ship
                        .fonts
                        .get(i)
                        .map(|p| (p.size, p.has_space))
                        .hash(s)),
                    h(&|s| t.pdf.ship.fonts.get(i).map(|p| &p.map).hash(s)),
                    h(&|s| t.pdf.ship.fonts.get(i).map(|p| &p.font_type).hash(s)),
                ]
            };
            let names = [
                "loaded", "ident", "params", "hyphen", "codes", "expand", "name", "pdf used",
                "pdf num", "pdf size", "pdf map", "pdf type",
            ];
            let (a, b) = (part(self), part(other));
            let which: alloc::vec::Vec<&str> = names
                .iter()
                .enumerate()
                .filter(|(k, _)| a.get(*k) != b.get(*k))
                .map(|(_, n)| *n)
                .collect();
            let about = |t: &Self| {
                let fs = &t.fonts;
                (
                    fs.rank.get(i).copied(),
                    fs.ident.get(i).map(|x| x % 1000),
                    alloc::string::String::from_utf8_lossy(
                        &t.string_bytes(fs.name.get(i).copied().unwrap_or(0)),
                    )
                    .into_owned(),
                    t.pdf.ship.fonts.get(i).map(|p| (p.used, p.num)),
                )
            };
            out.push(alloc::format!(
                "{f}: {which:?} {:?} / {:?}",
                about(self),
                about(other)
            ));
        }
        alloc::format!(
            "{} font slots differ: {:?}",
            out.len(),
            &out[..out.len().min(12)]
        )
    }

    /// The registers above 255 and the fonts that differ from `other`'s
    /// (for the convergence probe).
    #[must_use]
    pub fn xregs_fonts_difference(&self, other: &Self) -> alloc::string::String {
        let x: alloc::collections::BTreeMap<i32, (u64, i32)> = self
            .xregs
            .cells()
            .map(|(l, w, v)| (l, (w.bits(), v)))
            .collect();
        let y: alloc::collections::BTreeMap<i32, (u64, i32)> = other
            .xregs
            .cells()
            .map(|(l, w, v)| (l, (w.bits(), v)))
            .collect();
        let mut xd = alloc::vec::Vec::new();
        for (l, a) in &x {
            if y.get(l) != Some(a) {
                xd.push(alloc::format!("{l} {a:?}/{:?}", y.get(l)));
            }
        }
        for l in y.keys() {
            if !x.contains_key(l) {
                xd.push(alloc::format!("{l} none/{:?}", y.get(l)));
            }
        }
        let (f, g) = (&self.fonts, &other.fonts);
        let mut fd = alloc::vec::Vec::new();
        for i in 0..f.used.len().max(g.used.len()) {
            let a = (
                f.used.get(i),
                f.hyphen_char.get(i),
                f.skew_char.get(i),
                f.glue.get(i).map(Option::is_some),
            );
            let b = (
                g.used.get(i),
                g.hyphen_char.get(i),
                g.skew_char.get(i),
                g.glue.get(i).map(Option::is_some),
            );
            if a != b {
                let name = |t: &Self| {
                    let n = t.fonts.name.get(i).copied().unwrap_or(0);
                    let s = usize::try_from(n).unwrap_or(0);
                    if s == 0 || s >= t.str_ptr {
                        return alloc::string::String::new();
                    }
                    let (pool, start) = (t.str_pool.to_vec(), t.str_start.to_vec());
                    let size = t.fonts.metrics.get(i).map_or(0, |m| m.size);
                    alloc::format!(
                        "{}@{size}",
                        alloc::string::String::from_utf8_lossy(&pool[start[s]..start[s + 1]])
                    )
                };
                fd.push(alloc::format!(
                    "{i} {} {a:?}/{} {b:?}",
                    name(self),
                    name(other)
                ));
            }
        }
        alloc::format!(
            "font_ptr {} {} xregs {} {:?} fonts {} {:?}",
            self.font_ptr,
            other.font_ptr,
            xd.len(),
            &xd[..xd.len().min(8)],
            fd.len(),
            &fd[..fd.len().min(12)]
        )
    }

    /// [`Tex::cs_debug`] of eqtb location `p`.
    #[must_use]
    pub fn cs_debug_at(&self, p: i32) -> alloc::string::String {
        let w = self.peek_eqtb(p);
        let body = if (crate::cmds::CALL..=crate::cmds::LONG_OUTER_CALL).contains(&w.b0()) {
            let toks = self
                .peek_obj(p)
                .and_then(Obj::toks)
                .map_or(&[][..], |t| t.tokens());
            alloc::format!("{:?}", &toks[..toks.len().min(12)])
        } else {
            alloc::string::String::new()
        };
        alloc::format!(
            "({}, {}, {}) name {:?} level {:?} {:x} {body}",
            w.b0(),
            w.b1(),
            w.rh(),
            alloc::string::String::from_utf8_lossy(&self.slot_name(p)),
            (p >= INT_BASE).then(|| self.peek_xeq_level(p)),
            self.mcell_content(p) % (1 << 32)
        )
    }

    /// Control sequence `name`'s location, word and content hash, and
    /// its macro body's tokens if it is one (for the convergence probe).
    #[must_use]
    pub fn cs_debug(&self, name: &[u8]) -> alloc::string::String {
        let p = self.name_location(name);
        if p == 0 {
            return alloc::string::String::from("undefined");
        }
        let w = self.peek_eqtb(p);
        let body = if (crate::cmds::CALL..=crate::cmds::LONG_OUTER_CALL).contains(&w.b0()) {
            let toks = self
                .peek_obj(p)
                .and_then(Obj::toks)
                .map_or(&[][..], |t| t.tokens());
            alloc::format!("{toks:?}")
        } else {
            alloc::string::String::new()
        };
        alloc::format!(
            "{p} ({}, {}) {:x} {body}",
            w.b0(),
            w.rh(),
            self.mcell_content(p) % (1 << 32)
        )
    }

    /// A readable name of eqtb location `loc` (for `PARTEX_WATCH_DEBUG`).
    #[must_use]
    pub fn eqtb_loc_name(&self, loc: i32) -> alloc::string::String {
        use partex_engine::web::{
            BOX_BASE, CAT_CODE_BASE, COUNT_BASE, DIMEN_BASE, GLUE_BASE, HASH_BASE, INT_BASE,
            LOCAL_BASE, SCALED_BASE, SKIP_BASE, TOKS_BASE, UNDEFINED_CONTROL_SEQUENCE,
        };
        let extra = loc > crate::eqtb::EQTB_SIZE && loc < crate::xregs::EXT_BASE;
        if (HASH_BASE..UNDEFINED_CONTROL_SEQUENCE).contains(&loc) || extra {
            let t = self.hash[usize::try_from(loc - HASH_BASE).unwrap_or(0)].rh();
            if let Ok(t) = usize::try_from(t)
                && t > 0
                && t < self.str_ptr
            {
                let (pool, start) = (self.str_pool.to_vec(), self.str_start.to_vec());
                let b = &pool[start[t]..start[t + 1]];
                return alloc::format!("\\{}", alloc::string::String::from_utf8_lossy(b));
            }
        }
        let regions = [
            (crate::xregs::EXT_BASE, "register above 255"),
            (crate::eqtb::EQTB_SIZE + 1, "control sequence (hash_extra)"),
            (SCALED_BASE, "dimen"),
            (DIMEN_BASE, "dimen parameter"),
            (COUNT_BASE, "count"),
            (INT_BASE, "integer parameter"),
            (CAT_CODE_BASE, "codes"),
            (BOX_BASE, "box"),
            (TOKS_BASE, "toks"),
            (LOCAL_BASE, "local"),
            (SKIP_BASE, "skip"),
            (GLUE_BASE, "glue parameter"),
            (HASH_BASE, "control sequence"),
            (0, "single or active"),
        ];
        let (base, what) = regions
            .iter()
            .find(|(b, _)| loc >= *b)
            .copied()
            .unwrap_or((0, "?"));
        if base == COUNT_BASE {
            // (the control sequences that name the register)
            let names: alloc::vec::Vec<alloc::string::String> = (HASH_BASE
                ..UNDEFINED_CONTROL_SEQUENCE)
                .chain(
                    crate::eqtb::EQTB_SIZE + 1
                        ..crate::xregs::EXT_BASE.min(i32::try_from(self.eqtb.len()).unwrap_or(0)),
                )
                .filter(|&p| {
                    self.eq_type(p) == partex_engine::web::ASSIGN_INT && self.equiv(p) == loc
                })
                .map(|p| self.eqtb_loc_name(p))
                .take(3)
                .collect();
            return alloc::format!("{what} {} {names:?}", loc - base);
        }
        alloc::format!("{what} {}", loc - base)
    }

    /// [`Tex::state_hash`], and its parts into `parts`.
    fn state_hash_into(&self, parts: Option<&mut alloc::vec::Vec<(&'static str, u128)>>) -> u128 {
        self.state_hash_with(parts, Scope::All)
    }

    /// The state's parts as [`Tex::state_hash_parts`] gives them, but
    /// without the tracked cells (eqtb, the hash, registers above 255):
    /// what the sanitizer compares besides the cells (`sanitize.rs`). The
    /// save stack, levels and everything else stay in.
    #[must_use]
    pub fn untracked_state_parts(&self) -> alloc::vec::Vec<(&'static str, u128)> {
        let mut parts = alloc::vec::Vec::new();
        self.state_hash_with(Some(&mut parts), Scope::Untracked);
        parts
    }

    /// The hash of everything but eqtb's words (below the registers above
    /// 255): the `Rest` cell of the engine as a `partex_incr::Machine`
    /// (`machine.rs`), whose other cells are those words. Files being
    /// read contribute what is left of them, by content, so two states
    /// with equal hashes read on alike whatever came before.
    #[must_use]
    pub fn rest_hash(&self) -> u128 {
        self.rest_hash_of(self.state_hash_with(None, Scope::Rest), None)
    }

    /// [`Tex::rest_hash`], after a snapshot (`commit`), with the sub-hashes
    /// of what did not change since the last call remembered
    /// (`hashmemo.rs`): what it costs is what changed.
    ///
    /// Files for which `served` holds are cells of their own (by lines):
    /// they count by the lines read, not by what is left of them.
    pub fn rest_hash_memo(&mut self, served: &dyn Fn(&[u8]) -> bool) -> u128 {
        self.commit();
        let mut memo = core::mem::take(&mut self.hash_memo);
        memo.begin();
        let h = self.state_hash_timed(None, Scope::Rest, Some((&mut memo, served)));
        memo.end();
        self.hash_memo = memo;
        self.rest_hash_of(h, Some(served))
    }

    /// [`Tex::rest_hash_memo`] without the memo (the same value).
    #[must_use]
    pub fn rest_hash_served(&self, served: &dyn Fn(&[u8]) -> bool) -> u128 {
        let mut memo = crate::hashmemo::HashMemo::default();
        let h = self.state_hash_timed(None, Scope::Rest, Some((&mut memo, served)));
        self.rest_hash_of(h, Some(served))
    }

    fn rest_hash_of(&self, state: u128, served: Option<&Served<'_>>) -> u128 {
        let mut h = StableHasher::new();
        h.write_u128(state);
        for f in self.input_file.iter().chain(&self.read_file) {
            match f {
                Some(f) if served.is_some_and(|s| s(&f.name)) => h.write_u8(2),
                Some(f) => {
                    h.write_u8(1);
                    h.write(&f.data[f.pos.min(f.data.len())..]);
                }
                None => h.write_u8(0),
            }
        }
        h.finish128()
    }

    /// The parts of [`Tex::rest_hash`] (for debugging the machine).
    #[must_use]
    pub fn rest_hash_parts(&self) -> alloc::vec::Vec<(&'static str, u128)> {
        let mut parts = alloc::vec::Vec::new();
        self.state_hash_with(Some(&mut parts), Scope::Rest);
        parts
    }

    /// [`Tex::state_hash_into`], with the tracked cells or without.
    #[allow(clippy::too_many_lines)]
    fn state_hash_with(
        &self,
        parts: Option<&mut alloc::vec::Vec<(&'static str, u128)>>,
        scope: Scope,
    ) -> u128 {
        self.state_hash_timed(parts, scope, None)
    }

    fn state_hash_timed(
        &self,
        mut parts: Option<&mut alloc::vec::Vec<(&'static str, u128)>>,
        scope: Scope,
        memo: Option<(&mut crate::hashmemo::HashMemo, &Served<'_>)>,
    ) -> u128 {
        let cells = scope != Scope::Untracked;
        let parts = &mut parts;
        let Tex {
            // not state: the host and tracker are the caller's; the
            // parameters are the same for every run of a session
            unicode: _,
            doing_special: _,
            name_scratch: _,
            xfont: _,
            file_name_quote_char: _,
            host: _,
            tracker: _,
            params: _,
            // (a value beside the input stack and buffer, DESIGN 7.17.13
            // item 1)
            input_values: _,
            xord,
            xchr,
            xprn,
            str_pool,
            str_start,
            pool_ptr,
            str_ptr,
            init_pool_ptr,
            init_str_ptr,
            str_index: _,
            skip: _,
            // output already written or buffered: effects, not state (the
            // splice carries them); which files are open is state
            log_file,
            term_buf: _,
            write_file,
            selector,
            // (`dig` is scratch, filled before each use: after a byte count
            // it holds the engine's own guess of a length, which the link
            // corrects, `effects.rs`)
            dig: _,
            // a count of characters printed, read only after it is reset
            // (§292, §316, §318): dead between, so not state (a `\write`
            // of another length leaves it different)
            tally: _,
            term_offset,
            file_offset,
            flow: _,
            trick_buf,
            trick_count,
            first_count,
            arith_error,
            save_arith_error,
            remainder,
            line,
            buffer,
            first,
            last,
            // statistics, not emulated
            max_buf_stack: _,
            interaction,
            deletions_allowed,
            set_box_allowed,
            history,
            error_count,
            help_line,
            help_ptr,
            use_err_help,
            special_printing,
            message_printing,
            no_convert,
            active_noconvert,
            cs_converting,
            font_in_short_display,
            depth_threshold,
            breadth_max,
            nest,
            max_nest_stack: _,
            cur_list,
            shown_mode,
            eqtb,
            eqtb_obj,
            glue_lineage: _,
            xeq_level,
            eqtb_top,
            old_setting,
            sys_time,
            sys_day,
            sys_month,
            sys_year,
            hash,
            hash_used,
            hash_top,
            hash_high,
            no_new_control_sequence,
            cs_count: _,
            cur_val,
            cur_glue,
            glue_origin,
            cur_toks: _,
            split_discards,
            par_loc,
            par_token,
            write_loc,
            mltex_enabled_p,
            enctex_enabled_p,
            interrupt,
            ok_to_interrupt,
            halting_on_error,
            edit_request,
            save_stack,
            save_ptr,
            save_eqtb,
            save_obj,
            max_save_stack: _,
            cur_level,
            cur_group,
            cur_boundary,
            mag_set,
            cur_cmd,
            cur_chr,
            cur_cs,
            cur_tok,
            input_stack,
            input_ptr,
            max_in_stack: _,
            cur_input,
            in_open,
            open_parens,
            synctex_tags,
            synctex_flags,
            synctex_root,
            synctex_shipped,
            input_file,
            line_stack,
            grp_stack,
            if_stack,
            eof_seen,
            pseudo_files,
            source_filename_stack,
            full_source_filename_stack,
            scanner_status,
            warning_index,
            def_ref,
            def_protected: _,
            param_stack,
            param_ptr,
            max_param_stack: _,
            align_state,
            base_ptr,
            expand_depth_count,
            cur_val_level,
            radix,
            cur_order,
            dead_cycles,
            out_rtl,
            lr_problems,
            last_badness,
            output_active,
            read_file,
            read_open,
            cur_name,
            cur_area,
            cur_ext,
            area_delimiter,
            ext_delimiter,
            quoted_filename,
            stop_at_space,
            name_of_file,
            format_ident,
            // (not state: the format loaded, the same for every run)
            format_data: _,
            force_eof,
            long_state,
            cond_stack,
            if_limit,
            cur_if,
            if_line,
            skip_line,
            cur_mark,
            job_name,
            log_opened,
            name_in_progress,
            output_file_name,
            log_name,
            fmem_ptr: _,
            font_ptr,
            dvi,
            write_open,
            pack_begin_line,
            adjust,
            hyph,
            page,
            align,
            etex_mode,
            epoch,
            is_in_csname,
            xregs,
            max_reg_num,
            max_reg_help_line,
            tok_pool: _,
            pstack_buf: _,
            empty_list: _,
            omit_list: _,
            arg_list,
            arg_active,
            // (set only inside a scan)
            token_only: _,
            preamble_list,
            preamble_active,
            prims,
            random,
            cur_box,
            after_token,
            memo: _,
            cs_cache: _,
            map_cache: _,
            hash_memo: _,
            line_log: _,
            log_lines: _,
            glyphs_used: _,
            record_objstms: _,
            objstms_written: _,
            glyphs_read: _,
            // (the sealed lines are cells of their own; the log is
            // scratch; where to stop is scheduling)
            // (derived from eqtb; scheduling; scratch)
            skip_tracked: _,
            class_hash: _,
            classes_read: _,
            classes_written: _,
            seals: _,
            seal_lines,
            canon_strings,
            cs_by_name,
            // (a switch, and scratch)
            name_cells,
            name_log: _,
            font_cells,
            font_log: _,
            // (a switch: where names are, not what)
            probe_names: _,
            seal_at,
            seal_log: _,
            stop_before_ship: _,
            ship_stop,
            // (SSA mode's switch and stop, never a machine's)
            stop_after_load: _,
            load_stop: _,
            defer_page: _,
            page_pending: _,
            graf_stop: _,
            step_began: _,
            par_start,
            fire_pending,
            // (a switch)
            defer_fire: _,
            // (scheduling: where windows end, DESIGN 4.3 item 1)
            window: _,
            window_start: _,
            window_cut: _,
            fresh_def: _,
            long_help_seen,
            cancel_boundary,
            // scheduling, not state
            checkpoint_every: _,
            commands: _,
            stop_at: _,
            stop_at_candidate: _,
            at_checkpoint: _,
            checkpoint_at: _,
            block_entered: _,
            dense: _,
            dense_at: _,
            dense_last: _,
            shipped: _,
            tounicode,
            pdf,
            fontmap,
            fonts_mapped,
            // (the files the streams store to and the contents' versions
            // of those read: `write_file` and `read_file` above hold them)
            streams: _,
            fonts,
            diag,
            // (outputs, not state; with them, the DVI file's position is
            // state, `DviState::hash_state`)
            effects,
            // (glyph origins, `SyncTeX` and display lists: side channels,
            // not state)
            org: _,
            sync: _,
            dl: _,
        } = self;
        let mut c = Canon::new();
        if let Some((m, s)) = memo {
            c.memo = Some(m);
            c.served = Some(s);
        }
        let canon = *canon_strings && scope == Scope::Rest;
        if canon {
            c.strs = Some(Strs {
                pool: str_pool,
                start: str_start,
                init: *init_str_ptr,
                ptr: *str_ptr,
            });
        }
        // Tables: eqtb, the hash, registers above 255, the save stack.
        // (most of eqtb and the hash is unused: only what differs from
        // undefined is hashed, with its location)
        c.put(&(eqtb.len(), *eqtb_top));
        let undefined = eqtb[usize::try_from(UNDEFINED_CONTROL_SEQUENCE).expect("eqtb")].bits();
        if scope == Scope::All {
            for (l, &w) in (0..).zip(eqtb.slices().flatten()) {
                if (INT_BASE..=EQTB_SIZE).contains(&l) || w.bits() != undefined {
                    c.put(&l);
                    let o = eqtb_obj
                        .get(usize::try_from(l).unwrap_or(0))
                        .and_then(Option::as_ref);
                    c.word(l, w, o);
                }
            }
        }
        if scope != Scope::Rest {
            // (in `Rest` scope a level is part of its eqtb word's value)
            c.put(xeq_level);
        }
        c.section("tables: eqtb", parts);
        c.put(&hash.len());
        // (with names as cells, a machine's, the slots' names and links
        // are cells, and how many of `hash_extra`'s places are taken a
        // count TeX observes only when they are all taken)
        let names = *name_cells && scope == Scope::Rest;
        if cells && !names {
            c.names(hash);
        }
        if names && *cs_by_name {
            c.put(&(*hash_used, *hash_top, *no_new_control_sequence));
        } else {
            c.put(&(*hash_used, *hash_top, *hash_high, *no_new_control_sequence));
        }
        c.section("tables: hash", parts);
        if scope != Scope::Rest {
            // (in `Rest` scope the registers above 255 are cells, as
            // eqtb's words are)
            for (loc, w, level) in xregs.cells() {
                c.put(&level);
                if cells {
                    c.ext_word(loc, w, xregs.obj(loc));
                }
            }
        }
        c.put(&(xregs.chain_level, xregs.chain.len()));
        for s in &xregs.chain {
            c.saved(s);
        }
        for chain in &xregs.outer {
            c.put(&chain.len());
            for s in chain {
                c.saved(s);
            }
        }
        c.section("tables: xregs", parts);
        c.put(save_ptr);
        let top = usize::try_from(*save_ptr).unwrap_or(0);
        for p in 0..top {
            if save_eqtb.get(p).copied().unwrap_or(false) {
                // the saved value of the location in the entry above it
                let loc = save_stack[p + 1].rh();
                c.word(loc, save_stack[p], save_obj.get(p).and_then(Option::as_ref));
            } else {
                c.put(&save_stack[p].bits());
            }
        }
        c.put(&(*cur_level, *cur_group, *cur_boundary, *mag_set));
        c.section("tables", parts);

        // Strings and fonts.
        if canon {
            // (the format's strings as they are; the run's as a multiset,
            // references to them hashed by contents: the order the run
            // made them in is not state)
            c.segments(str_pool, *init_pool_ptr, |m| Some(&mut m.bytes));
            c.segments(str_start, *init_str_ptr + 1, |m| Some(&mut m.sizes));
            let open = str_pool.range_all(str_start.get_all(*str_ptr), *pool_ptr);
            if names {
                // (with names as cells, the run's strings matter only
                // through what holds them, hashed by their characters:
                // the names, and the references here; the string being
                // made is state)
                c.put(&*open);
            } else {
                let mut sum = 0u128;
                for s in *init_str_ptr..*str_ptr {
                    let b = str_pool.range_all(str_start.get_all(s), str_start.get_all(s + 1));
                    sum = sum.wrapping_add(StableHasher::of(&*b));
                }
                c.put(&(sum, *str_ptr, *pool_ptr, &*open));
            }
        } else {
            c.segments(str_pool, *pool_ptr, |m| Some(&mut m.bytes));
            c.segments(str_start, *str_ptr + 1, |m| Some(&mut m.sizes));
        }
        c.put(&(*init_pool_ptr, *init_str_ptr));
        c.section("strings", parts);
        // (the fonts' state and the font table are cells too)
        if *font_cells && scope == Scope::Rest {
            // (with fonts as cells, a machine's: each slot, the fonts by
            // name, the expanded fonts and the order of loading are cells;
            // what stays is the metrics of fonts whose characters' tags
            // changed, which reads of a font's metrics do not tell)
            for (i, &r) in fonts.retagged.iter().enumerate() {
                if r && fonts.rank.get(i).is_some_and(|&k| k > 0) {
                    let h = c.shared(&fonts.metrics[i], |m| &mut m.fonts);
                    c.put(&(i, h));
                }
            }
            c.section("fonts: count", parts);
            c.section("fonts: metrics", parts);
        } else if cells {
            c.put(&(*font_ptr, fonts.metrics.len()));
            c.section("fonts: count", parts);
            for f in &fonts.metrics {
                let h = c.shared(f, |m| &mut m.fonts);
                c.put(&h);
            }
            for (&n, &a) in fonts.name.iter().zip(&fonts.area) {
                c.sref(n);
                c.sref(a);
            }
            c.section("fonts: metrics", parts);
            c.put(&(fonts.name.len(), fonts.area.len()));
            if scope != Scope::Rest {
                // (in `Rest` scope the interword glue is left out: a cache,
                // §1042, of the font's parameters 2–4, emptied when one is
                // set; a font used for text earlier in one run than in
                // another is the same state)
                c.put(&fonts.glue);
            }
            c.put(&(
                &fonts.used,
                &fonts.hyphen_char,
                &fonts.skew_char,
                &fonts.codes,
                &fonts.expand,
                &fonts.expanded,
                &fonts.order,
            ));
        } else {
            c.put(&fonts.used);
        }
        if !(*font_cells && scope == Scope::Rest) {
            c.put(
                &fonts
                    .tfm
                    .iter()
                    .map(|t| t.len())
                    .collect::<alloc::vec::Vec<_>>(),
            );
        }
        c.section("strings and fonts", parts);

        // Input: the stack, the files and what is left of them.
        for r in &input_stack[..*input_ptr] {
            c.input(r);
        }
        c.input(cur_input);
        c.put(&(*input_ptr, *in_open, *open_parens, *base_ptr));
        c.put(&(
            synctex_tags,
            synctex_flags,
            synctex_root.as_deref(),
            synctex_shipped,
        ));
        c.section("input: stack", parts);
        // (with positions as cells, a machine's `Rest` has no line
        // numbers of the input: they are `MCell::Positions`'s)
        let positions = scope == Scope::Rest
            && c.served.is_some()
            && POSITION_CELLS.load(core::sync::atomic::Ordering::Relaxed);
        c.put(&buffer.prefix((*first).max(*last)));
        c.put(&(*first, *last, if positions { 0 } else { *line }));
        c.section("input: buffer", parts);
        let served = c.served;
        let left = |f: &Option<crate::input::AlphaFile>| {
            f.as_ref().map(|f| match served {
                Some(s) if s(&f.name) => (0, if positions { 0 } else { f.lines }),
                _ => (1, u32::try_from(f.data.len() - f.pos).unwrap_or(u32::MAX)),
            })
        };
        c.put(&input_file.iter().map(left).collect::<alloc::vec::Vec<_>>());
        if cells {
            c.put(&read_file.iter().map(left).collect::<alloc::vec::Vec<_>>());
            c.put(read_open);
        }
        if scope == Scope::Rest {
            // (entries above the open files are dead: set before they are
            // read, when a file opens at that level)
            let live = (*in_open + 1).min(line_stack.len());
            c.put(&(
                &line_stack[..if positions { 0 } else { live }],
                &grp_stack[..live],
                &if_stack[..live],
                &eof_seen[..live],
                pseudo_files,
            ));
            for &s in source_filename_stack[..live]
                .iter()
                .chain(&full_source_filename_stack[..live])
            {
                c.sref(s);
            }
        } else {
            c.put(&(line_stack, grp_stack, if_stack, eof_seen, pseudo_files));
            c.put(&(source_filename_stack, full_source_filename_stack));
        }
        c.section("input: files", parts);
        c.put(&(*scanner_status, *warning_index, *align_state, *force_eof));
        if scope != Scope::Rest {
            // (between two commands `def_ref` is stale: the last body
            // defined, which eqtb owns; `Rest` leaves eqtb's lists out)
            c.put(def_ref);
        }
        c.section("input: scanner", parts);
        c.put(param_ptr);
        for p in &param_stack[..usize::try_from(*param_ptr).unwrap_or(0)] {
            c.tok(p.as_ref());
        }
        c.put(&(
            cond_stack,
            *if_limit,
            *cur_if,
            *if_line,
            *skip_line,
            *long_state,
        ));
        c.section("input: conditionals", parts);

        // Lists, pages, alignments.
        c.put(&(nest, cur_list, *shown_mode));
        c.section("lists", parts);
        // (with the page as a cell, a machine's `Rest` has no page:
        // `MCell::Page` holds it)
        if scope == Scope::Rest
            && c.served.is_some()
            && PAGE_CELLS.load(core::sync::atomic::Ordering::Relaxed)
        {
            c.put(&(split_discards, adjust, cur_box));
        } else {
            c.put(&(page, split_discards, adjust, cur_box));
        }
        c.section("page", parts);
        c.put(&(
            align.cur.tabskips.glue(),
            &align.cur.adjust,
            align.cur.align_state,
        ));
        for level in core::iter::once(&align.cur).chain(align.stack.iter().map(|(l, _)| l)) {
            c.put(&(
                level.columns.len(),
                level.cur_align,
                level.cur_span,
                level.cur_loop,
            ));
            c.put(&(level.tabskips.glue(), &level.adjust, level.align_state));
            for col in &level.columns {
                c.tok(Some(&col.u));
                c.tok(Some(&col.v));
                c.put(&(col.extra_info, &col.column));
            }
        }
        c.put(&(arg_list, *arg_active, preamble_list, *preamble_active));
        c.section("alignments", parts);
        c.put(cur_mark);
        c.section("marks", parts);
        let words = if canon {
            // (the exception words are strings the run may have made)
            let mut h = StableHasher::new();
            for (i, &w) in hyph.hyph_word.iter().enumerate() {
                if w != 0 {
                    (i, c.strs.map(|s| s.key(w))).hash(&mut h);
                }
            }
            h.finish128()
        } else {
            c.shared(&hyph.hyph_word, |m| &mut m.ints)
        };
        let shared = [
            c.shared(&hyph.patterns, |m| &mut m.patterns),
            words,
            c.shared(&hyph.hyph_link, |m| &mut m.ints),
        ];
        hyph.hash_state(&mut c.h, shared);
        if cells {
            c.put(&(prims, random));
        } else {
            c.put(prims);
        }

        // Output: which files are open, and the DVI file's state.
        c.put(&(log_file.id.map(|w| w.0), *log_opened));
        c.sref(*log_name);
        c.put(
            &write_file
                .iter()
                .map(|f| f.id.map(|w| w.0))
                .collect::<alloc::vec::Vec<_>>(),
        );
        if cells {
            c.put(write_open);
        }
        dvi.hash_state(&mut c.h, effects.is_some());
        c.sref(*job_name);
        c.sref(*output_file_name);
        c.put(&(*name_in_progress, *dead_cycles));
        c.section("hyphenation, output files, dvi", parts);

        // Scalars.
        c.put(&(xord, xchr, xprn, *selector));
        c.put(&(
            *term_offset,
            *file_offset,
            trick_buf,
            *trick_count,
            *first_count,
        ));
        c.put(&(*arith_error, *save_arith_error, *remainder));
        c.put(&(*interaction, *deletions_allowed, *set_box_allowed, *history));
        c.put(&(*error_count, help_line, *help_ptr, *use_err_help));
        c.put(&(*special_printing, *message_printing, *no_convert));
        c.put(&(*active_noconvert, *cs_converting, *font_in_short_display));
        c.put(&(*depth_threshold, *breadth_max, *old_setting));
        c.section("scalars: printing", parts);
        // (`sys_*` left out: the job start, read from the host where it is
        // printed, a tracked read; see `open_log_file`)
        let _ = (sys_time, sys_day, sys_month, sys_year);
        c.put(&(*cur_val, cur_glue));
        match glue_origin {
            Some(l) => c.lineage(*l),
            None => c.put(&u32::MAX),
        }
        c.section("scalars: cur_val", parts);
        c.put(&(
            *par_loc,
            *par_token,
            *write_loc,
            *mltex_enabled_p,
            *enctex_enabled_p,
        ));
        c.put(&(*interrupt, *ok_to_interrupt, *halting_on_error));
        match edit_request {
            Some((s, l)) => {
                c.sref(*s);
                c.put(l);
            }
            None => c.put(&0u8),
        }
        c.section("scalars: flags", parts);
        c.put(&(*cur_cmd, *cur_chr, *cur_cs, *cur_tok));
        c.put(&(*expand_depth_count, *cur_val_level, *radix, *cur_order));
        c.section("scalars: current token", parts);
        c.put(&(*out_rtl, *lr_problems, *last_badness, *output_active));
        c.sref(*cur_name);
        c.sref(*cur_area);
        c.sref(*cur_ext);
        c.put(&(*area_delimiter, *ext_delimiter));
        c.section("scalars: file names", parts);
        c.put(&(*quoted_filename, *stop_at_space, name_of_file));
        c.sref(*format_ident);
        c.put(&(*pack_begin_line, *etex_mode, epoch, *is_in_csname));
        c.put(&(*max_reg_num, max_reg_help_line));
        c.put(&(*after_token, *long_help_seen, *cancel_boundary, diag));
        c.put(&(*seal_lines, *seal_at, *ship_stop, *par_start, *fire_pending));
        c.section("scalars", parts);
        // (a persistent map: its hash is its version, made at each write)
        c.put(tounicode);
        c.put(&(fontmap, fonts_mapped));
        c.section("font maps", parts);
        if scope == Scope::Rest
            && c.served.is_some()
            && PDF_LAST_CELLS.load(core::sync::atomic::Ordering::Relaxed)
        {
            // (with them as cells, a machine's `Rest` has no `\pdflast…`
            // values: `MCell::PdfLast` holds them)
            c.put(&crate::pdf::WithoutLast(pdf));
        } else {
            c.put(pdf);
        }
        if scope != Scope::Rest || !*font_cells {
            // (with fonts as cells, what the writer keeps of them is
            // theirs)
            c.put(&pdf.ship.fonts.0);
        }
        if scope != Scope::Rest {
            // (in `Rest` scope the glyphs used are a cell of their own)
            for f in &pdf.ship.fonts {
                c.put(&f.chars);
            }
        }
        c.section("pdf", parts);
        c.sections.finish128()
    }
}

/// The files a job's output goes to (see [`Tex::output_files`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct OutputFiles {
    /// The log file.
    pub log: Option<crate::host::WriteId>,
    /// The DVI file, once a page is written to it (its writer holds
    /// bytes back: see [`Tex::dvi_writer`]).
    pub dvi: Option<crate::host::WriteId>,
    /// The PDF file, once open (TeX never reads it back; its offsets
    /// follow a splice: see [`Tex::relocate_output`]).
    pub pdf: Option<crate::host::WriteId>,
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Move the PDF writer's byte positions after a splice. `self` is a
    /// later checkpoint of a previous build, `old_gone` the
    /// [`Tex::output_written`] of that build's checkpoint where this run
    /// (`now`) met it, in the same state but for positions. The spliced
    /// file is this run's bytes up to `now`'s written count, then the
    /// previous build's from `old_gone` on, so a position at or past
    /// `old_gone` moves by the difference of the two counts; one before it
    /// is this run's (objects `now` has written keep its offsets).
    ///
    /// (The threshold is the written count, not [`Tex::output_offset`]:
    /// with bytes buffered at the meeting point, a later checkpoint can
    /// have written less than that offset, and taking `now`'s count for
    /// it made every later object's offset stale.)
    pub fn relocate_output(&mut self, now: &Self, old_gone: i64) {
        self.relocate_output_past(now, old_gone, None);
    }

    /// [`Tex::relocate_output`], positions from `extra.0` on moving by
    /// `extra.1` more (an object rendered again at another length there).
    pub fn relocate_output_past(&mut self, now: &Self, old_gone: i64, extra: Option<(i64, i64)>) {
        let delta = now.pdf.out.gone - old_gone;
        let more = |x: i64| extra.map_or(0, |(at, d)| if x >= at { d } else { 0 });
        for (k, e) in self.pdf.objs.tab.iter_mut().enumerate() {
            if !e.at_byte() {
                continue;
            }
            match now.pdf.objs.tab.get(k) {
                Some(n) if n.at_byte() => e.offset = n.offset,
                _ => e.offset += delta + more(e.offset),
            }
        }
        self.pdf.out.relocate(&now.pdf.out, old_gone, delta, &more);
    }

    /// Record each PDF object stream written, for the session to take
    /// ([`Tex::take_objstms_written`]).
    pub fn record_objstms(&mut self, on: bool) {
        self.record_objstms = on;
    }

    /// The object streams written since the last call.
    pub fn take_objstms_written(&mut self) -> alloc::vec::Vec<crate::pdf::out::ObjStmWritten> {
        core::mem::take(&mut self.objstms_written)
    }

    /// The bytes the PDF writer has written to its file (handed over or
    /// pending), not counting those buffered.
    #[must_use]
    pub fn output_written(&self) -> i64 {
        self.pdf.out.gone
    }

    /// The PDF objects written at a byte offset of the file: (table
    /// index, offset). For consistency checks (`PARTEX_CHECK_OFFSETS`).
    #[must_use]
    pub fn pdf_object_offsets(&self) -> alloc::vec::Vec<(usize, i64)> {
        self.pdf
            .objs
            .tab
            .iter()
            .enumerate()
            .filter(|(_, e)| e.at_byte())
            .map(|(k, e)| (k, e.offset))
            .collect()
    }

    /// Where the PDF writer is in its file.
    #[must_use]
    pub fn output_offset(&self) -> i64 {
        self.pdf.out.offset()
    }

    /// The log and DVI files. (At a checkpoint every other output has
    /// been handed to the host.)
    #[must_use]
    pub fn output_files(&self) -> OutputFiles {
        OutputFiles {
            log: self.log_file.id,
            dvi: self.dvi.file.filter(|_| self.dvi.writer.is_some()),
            pdf: self.pdf.out.file,
        }
    }

    /// Where the input is, cheaply: states that can be equal have equal
    /// keys. (The line, the file level, and how much of each open file
    /// is left.)
    #[must_use]
    pub fn position_key(&self) -> (i32, usize, alloc::vec::Vec<usize>) {
        let left = self
            .open_inputs()
            .iter()
            .map(|(d, p)| d.len() - p)
            .collect();
        (self.line, self.in_open, left)
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// The value rows of DESIGN 7.17.12 that are not tables, each hashed
    /// on its own, for check mode's comparison by row (`ssa.rs`): the
    /// tables (eqtb, `xeq_level`, the registers, the hash) are compared
    /// by the record's writes, scratch rows not at all. A row's hash is
    /// only compared with the same engine's at another moment of the same
    /// run, so ids are hashed as they are (the state hash's canonical
    /// numbering is not needed to tell a change).
    #[must_use]
    pub fn value_rows(&self) -> alloc::vec::Vec<(&'static str, u128)> {
        fn of<V: Hash + ?Sized>(v: &V) -> u128 {
            let mut h = StableHasher::new();
            v.hash(&mut h);
            h.finish128()
        }
        let mut rows = alloc::vec::Vec::with_capacity(48);
        // (the pool and the hash's allocator are tables: check mode tests
        // their versions at each read)
        // (the log's being open, each `\write` stream's file and each
        // `\openin` stream are values, `Out(n)` and `Read(n)`, and their
        // bytes and lines effects and stores: `streams`)
        rows.push(("line", of(&self.line)));
        rows.push((
            "buffer, first, last",
            of(&(
                self.buffer.prefix(self.first.max(self.last)),
                self.first,
                self.last,
            )),
        ));
        // (`cur_list`'s fields and the nest are values the tracker sees:
        // check mode compares a hit's writes of them with the body's)
        // (the alignment state, the page builder's fields and e-TeX's
        // `split_disc` are values the tracker sees, by field)
        // (the save stack and its pointers are values the tracker sees:
        // check mode compares a hit's writes of them with the body's)
        let mut h = StableHasher::new();
        for r in &self.input_stack[..self.input_ptr] {
            (r.state, r.index, r.start, r.loc, r.limit, r.name, &r.list).hash(&mut h);
        }
        let r = &self.cur_input;
        (
            r.state,
            r.index,
            r.start,
            r.loc,
            r.limit,
            r.name,
            &r.list,
            self.input_ptr,
        )
            .hash(&mut h);
        rows.push(("input_stack, input_ptr, cur_input", h.finish128()));
        // (the scalar rows are slots of the tables' convention: check mode
        // compares a hit's writes of them with the body's)
        rows.push((
            "in_open, input_file, line_stack",
            of(&(
                self.in_open,
                self.input_file
                    .iter()
                    .map(|f| f.as_ref().map(|f| (f.data.len(), f.pos, f.lines)))
                    .collect::<alloc::vec::Vec<_>>(),
                self.line_stack
                    .get(..=self.in_open.min(self.line_stack.len().saturating_sub(1))),
            )),
        ));
        let live = (self.in_open + 1).min(self.grp_stack.len());
        rows.push((
            "grp_stack, if_stack, eof_seen",
            of(&(
                self.grp_stack.get(..live),
                self.if_stack.get(..live),
                self.eof_seen.get(..live),
            )),
        ));
        rows.push(("pseudo_files", of(&self.pseudo_files)));
        rows.push((
            "source_filename_stack, full_source_filename_stack",
            of(&(
                self.source_filename_stack.get(..live),
                self.full_source_filename_stack.get(..live),
            )),
        ));
        let params = usize::try_from(self.param_ptr).unwrap_or(0);
        rows.push((
            "param_stack, param_ptr",
            of(&(&self.param_stack[..params], params)),
        ));
        // (the conditionals are a slot versioned at its writes, and
        // `skip_line` is scratch at a boundary: `pass_text` sets it before
        // its only reader, the runaway error inside the same skip, §494)
        // (the marks are a slot versioned at its writes)
        // (the fonts' fields and their table are values the tracker sees:
        // check mode compares a hit's writes of them with the body's)
        // (the DVI and PDF writers' tables, the glyphs' Unicode table and
        // the font map are values: `pdf::val`)
        // (hyphenation's patterns and exceptions are values the tracker
        // sees)
        // (the random generator is a value: `streams`)
        rows
    }
}
