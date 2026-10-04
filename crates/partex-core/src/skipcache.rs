//! Skipped conditional text, remembered (tex.web §494 `pass_text`).
//!
//! Skipping the false branch of a conditional reads token after token
//! until the matching `\fi`, `\else` or `\or`; in LaTeX and PGF documents
//! the same branches of the same macro bodies are skipped millions of
//! times. Where a skip starts at position `loc` of a macro body with no
//! conditional open, it ends at the same place with the same effect (the
//! terminating token, the change of `align_state`) as long as the body is
//! the same (its id's generation) and no control sequence has changed what
//! it is to a skip (`class`: a conditional of any kind, `\fi`, `\else` or
//! `\or`, an `\outer` macro or `\noexpand`'s marker, an alignment tab or
//! `\cr`); the eqtb writers count such changes in `epoch`. (LaTeX's
//! switches, `\let\if@x\iftrue`, change no class.) A skip that meets a macro
//! parameter, the end of the body, or anything that makes `get_next` act
//! is not remembered and runs as tex.web's does.
//!
//! Without memo recording, and with a tracker that records reads only if
//! it is the SSA build's, which a remembered skip tells the lookups the
//! skip makes token by token ([`Skip::reads`]); `Params::skip_cache` (the
//! CLI's `PARTEX_SKIPCACHE=0` turns it off).

use alloc::vec::Vec;

use crate::mem::NULL;
use crate::web::{
    CAR_RET, CS_TOKEN_FLAG, FI_CODE, FI_OR_ELSE, IF_TEST, LEFT_BRACE, MAC_PARAM, OUTER_CALL,
    RIGHT_BRACE, TAB_MARK,
};

/// Entries (direct-mapped).
const SLOTS: usize = 1 << 16;

/// An entry's `end` for a skip that must run as tex.web's.
const UNCACHEABLE: i32 = i32::MIN;

#[derive(Clone, Default)]
struct Entry {
    /// The list (held, so its address is not reused while remembered).
    list: Option<partex_engine::node::Tokens>,
    loc: i32,
    epoch: u64,
    /// The `loc` after the terminating token; `UNCACHEABLE` for a skip
    /// that must run as tex.web's.
    end: i32,
    cmd: i32,
    chr: i32,
    cs: i32,
    align: i32,
    /// The control sequences the skip looks up, each once, in the order
    /// it first meets them (a tracker's reads: [`Skip::reads`]).
    reads: Option<alloc::sync::Arc<[i32]>>,
}

/// The remembered skips.
pub(crate) struct SkipCache {
    /// Bumped when a control sequence's meaning changes to or from one a
    /// skip reacts to.
    pub epoch: u64,
    slots: Vec<Entry>,
}

impl Default for SkipCache {
    fn default() -> Self {
        Self {
            epoch: 1,
            slots: Vec::new(),
        }
    }
}

impl Clone for SkipCache {
    /// Empty: a copy of the engine (a checkpoint) starts afresh.
    fn clone(&self) -> Self {
        Self {
            epoch: self.epoch,
            slots: Vec::new(),
        }
    }
}

impl core::fmt::Debug for SkipCache {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SkipCache")
    }
}

/// A skip's outcome: where it ends, its terminating token, and how much
/// it changed `align_state`.
#[derive(Clone)]
pub(crate) struct Skip {
    pub end: i32,
    pub cmd: i32,
    pub chr: i32,
    pub cs: i32,
    pub align: i32,
    /// With a tracker that records reads (the SSA build's): the control
    /// sequences whose meanings the skip looks up, the terminating one
    /// included, each once, in the order it first meets them. A
    /// remembered skip makes these lookups again, so the tracker sees the
    /// reads the skip makes token by token (`Tex::skip_remembered`).
    pub reads: Option<alloc::sync::Arc<[i32]>>,
}

/// What a control sequence meaning (`cmd`, `chr`) is to a skip: a
/// conditional (of any kind: `\iftrue` and `\iffalse` skip alike), `\fi`,
/// `\else` or `\or`, something `get_next` acts on, an alignment tab or
/// `\cr`, or nothing (0).
#[inline]
pub(crate) fn class(c: i32, chr: i32) -> u8 {
    if c == IF_TEST {
        1
    } else if c == FI_OR_ELSE {
        if chr == FI_CODE { 2 } else { 3 }
    } else if c >= OUTER_CALL {
        4
    } else if (TAB_MARK..=CAR_RET).contains(&c) {
        5
    } else {
        0
    }
}

/// What a control sequence meaning is to a reader that only stores its
/// token (an argument's, a body's, an assignment's target): what `get_next`
/// acts on (\outer, an alignment's tab or `\cr`), what a skip counts
/// ([`class`]), a macro parameter (a definition's `#`), or nothing (0).
#[inline]
pub(crate) fn token_class(c: i32, chr: i32) -> u8 {
    match class(c, chr) {
        0 if c == MAC_PARAM => 6,
        k => k,
    }
}

fn slot(list: &partex_engine::node::Tokens, loc: i32) -> usize {
    let a = alloc::sync::Arc::as_ptr(list) as usize as u64;
    let h = (a << 16 ^ u64::from(loc.cast_unsigned())).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    usize::try_from(h >> 48).unwrap_or(0) & (SLOTS - 1)
}

impl SkipCache {
    /// The skip remembered: `Some(None)` if it must run as tex.web's.
    #[expect(
        clippy::option_option,
        reason = "not known, or known not to be remembered"
    )]
    pub(crate) fn get(&self, list: &partex_engine::node::Tokens, loc: i32) -> Option<Option<Skip>> {
        let e = self.slots.get(slot(list, loc))?;
        let same = e
            .list
            .as_ref()
            .is_some_and(|l| alloc::sync::Arc::ptr_eq(l, list));
        if !(same && e.loc == loc && e.epoch == self.epoch) {
            return None;
        }
        Some((e.end != UNCACHEABLE).then(|| Skip {
            end: e.end,
            cmd: e.cmd,
            chr: e.chr,
            cs: e.cs,
            align: e.align,
            reads: e.reads.clone(),
        }))
    }

    /// Remember the skip from (`list`, `loc`): `None` if it must run as
    /// tex.web's.
    pub(crate) fn put(&mut self, list: &partex_engine::node::Tokens, loc: i32, s: Option<Skip>) {
        let s = s.unwrap_or(Skip {
            end: UNCACHEABLE,
            cmd: 0,
            chr: 0,
            cs: 0,
            align: 0,
            reads: None,
        });
        if self.slots.is_empty() {
            self.slots = alloc::vec![Entry::default(); SLOTS];
        }
        self.slots[slot(list, loc)] = Entry {
            list: Some(list.clone()),
            loc,
            epoch: self.epoch,
            end: s.end,
            cmd: s.cmd,
            chr: s.chr,
            cs: s.cs,
            align: s.align,
            reads: s.reads,
        };
    }
}

/// Skip in `toks` from index `loc` with no conditional open, as
/// `pass_text` would: the outcome, or `None` if the skip leaves the list
/// or meets something `get_next` acts on or might (a parameter, an
/// `\outer` macro, `\noexpand`'s marker, a tab or `\cr`). `meaning(cs)` is
/// the control sequence's (`cmd`, `chr`).
pub(crate) fn scan(
    toks: &[i32],
    loc: i32,
    mut meaning: impl FnMut(i32) -> (i32, i32),
) -> Option<Skip> {
    let mut l = 0;
    let mut align = 0;
    let mut k = usize::try_from(loc).ok()?;
    while let Some(&t) = toks.get(k) {
        k += 1;
        if t >= CS_TOKEN_FLAG {
            let cs = t - CS_TOKEN_FLAG;
            let (c, chr) = meaning(cs);
            // (a tab or `\cr` acts where `align_state` is 0: what the skip
            // does would depend on its value)
            if c >= OUTER_CALL || (TAB_MARK..=CAR_RET).contains(&c) {
                return None;
            }
            if c == FI_OR_ELSE {
                if l == 0 {
                    let end = if k < toks.len() {
                        i32::try_from(k).ok()?
                    } else {
                        NULL
                    };
                    return Some(Skip {
                        end,
                        cmd: c,
                        chr,
                        cs,
                        align,
                        reads: None,
                    });
                }
                if chr == FI_CODE {
                    l -= 1;
                }
            } else if c == IF_TEST {
                l += 1;
            }
        } else {
            match t / 0o400 {
                LEFT_BRACE => align += 1,
                RIGHT_BRACE => align -= 1,
                c if (TAB_MARK..=CAR_RET).contains(&c) => return None, // (`OUT_PARAM` too)
                _ => {}
            }
        }
    }
    None
}
