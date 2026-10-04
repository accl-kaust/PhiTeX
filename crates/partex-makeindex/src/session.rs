//! makeindex's output as blocks, made again only where what they read
//! changed (partex's DESIGN 3.16, "Exactly the dirty work").
//!
//! A run scans the `.idx` files and sorts the entries as makeindex does:
//! its comparison count, its progress dots and its duplicate marks are in
//! the `.ilg`, so the sort is one call over every entry. The output is
//! then made entry by entry (genind.c's `make_entry`), each a *block*
//! that reads its entry, the entries its state names (the one before it,
//! the first and last page of the run of pages it continues, an open
//! range's start) and genind.c's state (the level, the open line, the
//! range and encapsulator flags), and writes the state and its bytes of
//! the `.ind`. A [`Session`] keeps each block by what it read: a block
//! whose entry and state read the same is not made again, its bytes and
//! its state are taken as they were. An entry's place in the input (its
//! line) is read only by a warning; a block that warned is always made
//! again, so the `.ilg` says what makeindex's would.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::ind::Gen;
use crate::{Field, Files, Mk, Outcome};

/// A block as made: which of the state's entries it set to its own, the
/// state it left (but those entries and the counts), its bytes and the
/// lines it counted.
#[derive(Clone)]
struct Block {
    begin: bool,
    the_end: bool,
    range: bool,
    level: usize,
    prev_level: usize,
    encap: Vec<u8>,
    prev_encap: Option<Vec<u8>>,
    in_range: bool,
    encap_range: bool,
    buff: Vec<u8>,
    line: Vec<u8>,
    ind_indent: i32,
    ind: Vec<u8>,
    lc: i32,
}

/// The blocks of a session's runs, by what they read: the last run's,
/// and this run's.
#[derive(Default)]
pub(crate) struct Memo {
    /// What the blocks are valid for (the program, the options, the
    /// style), and this run's program and arguments.
    sig: Vec<u8>,
    head: Vec<u8>,
    old: BTreeMap<Vec<u8>, Block>,
    new: BTreeMap<Vec<u8>, Block>,
    stats: Stats,
}

/// What a session's run did (for the report).
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// Nothing to start from (a first run, other options or style).
    pub fresh: bool,
    /// The blocks of the output, and those made.
    pub blocks: usize,
    pub run: usize,
}

/// makeindex run by a session: the same program, its output's blocks made
/// again only where what they read changed.
#[derive(Default)]
pub struct Session {
    memo: Option<Memo>,
}

impl Session {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Run makeindex with `args` (as [`crate::run`]), reusing the last
    /// run's blocks where what they read is the same.
    pub fn run(
        &mut self,
        args: &[Vec<u8>],
        version: &[u8],
        files: &mut dyn Files,
    ) -> (Outcome, Stats) {
        let mut memo = self.memo.take().unwrap_or_default();
        memo.old = core::mem::take(&mut memo.new);
        memo.stats = Stats::default();
        memo.head.clear();
        for a in args {
            put(&mut memo.head, a);
        }
        put(&mut memo.head, version);
        let (out, memo) = crate::run_with(args, version, files, Some(memo));
        let Some(memo) = memo else {
            return (out, Stats::default());
        };
        let stats = memo.stats;
        self.memo = Some(memo);
        (out, stats)
    }
}

/// `v`, length first, onto `out`.
fn put(out: &mut Vec<u8>, v: &[u8]) {
    out.extend_from_slice(&(v.len() as u64).to_le_bytes());
    out.extend_from_slice(v);
}

/// What a block reads of an entry: everything but its place in the input
/// (its file and line, read only by a warning).
fn put_entry(out: &mut Vec<u8>, e: Option<&Field>) {
    let Some(e) = e else {
        out.push(0);
        return;
    };
    out.push(1);
    for s in e.sf.iter().chain(&e.af) {
        put(out, s);
    }
    out.extend_from_slice(&e.group.to_le_bytes());
    put(out, &e.lpg);
    for p in &e.npg {
        out.extend_from_slice(&p.to_le_bytes());
    }
    out.extend_from_slice(&(e.count as u64).to_le_bytes());
    out.extend_from_slice(&e.typ.to_le_bytes());
    put(out, &e.encap);
}

impl Mk<'_> {
    /// The output's generation begins: a session's blocks are valid only
    /// for the same options, style and program.
    pub(crate) fn gen_start(&mut self) {
        let Some(memo) = self.memo.as_mut() else {
            return;
        };
        let mut sig = memo.head.clone();
        put(&mut sig, &self.sty.data);
        put(
            &mut sig,
            &[
                u8::from(self.letter_ordering),
                u8::from(self.compress_blanks),
                u8::from(self.merge_page),
                u8::from(self.init_page),
                u8::from(self.german_sort),
                u8::from(self.thai_sort),
                u8::from(self.locale_sort),
            ],
        );
        sig.extend_from_slice(&self.even_odd.to_le_bytes());
        put(&mut sig, &self.pageno);
        if memo.sig != sig {
            memo.old.clear();
            memo.stats.fresh = true;
        }
        memo.sig = sig;
    }

    /// What the block of the `n`th sorted entry reads, as a key.
    fn block_key(&self, g: &Gen, n: usize) -> Vec<u8> {
        let mut k = Vec::new();
        k.push(u8::from(n == 0));
        let e = |i: Option<usize>| i.and_then(|i| self.entries.get(i));
        put_entry(&mut k, self.entries.get(self.idx_key[n]));
        for r in [g.curr, g.begin, g.the_end, g.range_ptr] {
            put_entry(&mut k, e(r));
        }
        k.extend_from_slice(&(g.level as u64).to_le_bytes());
        match &g.prev_encap {
            Some(p) => {
                k.push(1);
                put(&mut k, p);
            }
            None => k.push(0),
        }
        k.push(u8::from(g.in_range));
        k.push(u8::from(g.encap_range));
        put(&mut k, &g.line);
        k.extend_from_slice(&g.ind_indent.to_le_bytes());
        k
    }

    /// The block of the `n`th sorted entry: taken as it was if what it
    /// reads is the same, else made (`make_entry`).
    pub(crate) fn make_block(&mut self, g: &mut Gen, n: usize) {
        if self.memo.is_none() {
            self.make_entry(g, n);
            return;
        }
        let key = self.block_key(g, n);
        let c = self.idx_key[n];
        let memo = self.memo.as_mut().expect("a session");
        memo.stats.blocks += 1;
        if let Some(b) = memo.old.get(&key).or_else(|| memo.new.get(&key)).cloned() {
            g.prev = g.curr;
            g.curr = Some(c);
            if b.begin {
                g.begin = Some(c);
            }
            if b.the_end {
                g.the_end = Some(c);
            }
            if b.range {
                g.range_ptr = Some(c);
            }
            g.level = b.level;
            g.prev_level = b.prev_level;
            g.encap.clone_from(&b.encap);
            g.prev_encap.clone_from(&b.prev_encap);
            g.in_range = b.in_range;
            g.encap_range = b.encap_range;
            g.buff.clone_from(&b.buff);
            g.line.clone_from(&b.line);
            g.ind_indent = b.ind_indent;
            g.ind_lc += b.lc;
            self.ind.extend_from_slice(&b.ind);
            memo.new.insert(key, b);
            return;
        }
        memo.stats.run += 1;
        let (ind0, lc0, ec0) = (self.ind.len(), g.ind_lc, g.ind_ec);
        let (begin0, end0, range0) = (g.begin, g.the_end, g.range_ptr);
        self.make_entry(g, n);
        // (kept only if it did not warn, and each entry its state names is
        // the one it read or its own)
        let own = |now: Option<usize>, was: Option<usize>| -> Option<bool> {
            if now == Some(c) && was != Some(c) {
                Some(true)
            } else if now == was {
                Some(false)
            } else {
                None
            }
        };
        let (Some(begin), Some(the_end), Some(range)) = (
            own(g.begin, begin0),
            own(g.the_end, end0),
            own(g.range_ptr, range0),
        ) else {
            return;
        };
        if g.ind_ec != ec0 {
            return;
        }
        let b = Block {
            begin,
            the_end,
            range,
            level: g.level,
            prev_level: g.prev_level,
            encap: g.encap.clone(),
            prev_encap: g.prev_encap.clone(),
            in_range: g.in_range,
            encap_range: g.encap_range,
            buff: g.buff.clone(),
            line: g.line.clone(),
            ind_indent: g.ind_indent,
            ind: self.ind[ind0..].to_vec(),
            lc: g.ind_lc - lc0,
        };
        if let Some(m) = self.memo.as_mut() {
            m.new.insert(key, b);
        }
    }
}
