//! Token lists as values (tex.web §200, §289–§291, §371; DESIGN §7.17.12).
//!
//! A token list is a [`Tokens`]: an immutable [`TokenList`] shared by
//! `Arc`, carrying the version it was made with (partex-engine's
//! `node.rs`). The tables that hold one (eqtb entries, the save stack,
//! marks and `\write` texts, alignment templates) and the input levels
//! that read one hold the value itself, never an address of its own: a
//! list lives while something holds it, which is tex.web's reference
//! count (§200) with the input stack's references counted too, so a list
//! a table dropped stays while a level reads it (§324's behaviour). A
//! list being built is a vector (`def_ref`, an argument, a template) and
//! becomes a value when it is made.
//!
//! Macro arguments and backed-up tokens are made by the tens of millions
//! (59M arguments and as many backed-up tokens on the course). A list no
//! one holds any more goes back to a pool (`Tex::tok_pool`) and its
//! allocation is filled again for the next one; the pool is an
//! allocator's concern, not state (a list from it is a new value).

use alloc::sync::Arc;
use alloc::vec::Vec;

pub(crate) use partex_engine::node::{TokenList, Tokens};

use crate::host::Host;
use crate::tex::Tex;
use crate::track::Tracker;
use crate::web::END_TEMPLATE_TOKEN;

/// Whether released lists are reused (`PARTEX_PARAM_ARENA=0`: off, each
/// list its own allocation; the output is the same either way).
static POOL: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Turn [`POOL`] on or off for the process.
pub fn set_arg_pool(on: bool) {
    POOL.store(on, core::sync::atomic::Ordering::Relaxed);
}

/// Kept for the command line's switch; lists carry no change marks any
/// more (a list is a value).
pub fn set_changed_ids(_on: bool) {}

/// The most lists the pool keeps.
const POOL_MAX: usize = 1024;

impl<H: Host, T: Tracker> Tex<H, T> {
    /// A new list of `toks`, made now.
    #[must_use]
    #[allow(clippy::unused_self, reason = "the engine's allocator of lists")]
    pub(crate) fn make_list(&self, toks: Vec<i32>) -> Tokens {
        Arc::new(TokenList::new(toks, false))
    }

    /// A new list holding `toks` (from the pool: an inserted or backed-up
    /// list, given back when its level ends).
    pub(crate) fn tok_from(&mut self, toks: &[i32]) -> Tokens {
        self.pooled_list(|b| b.extend_from_slice(toks))
    }

    /// A new list filled by `fill`, in an allocation from the pool (a
    /// macro argument, a backed-up token, an inserted one).
    pub(crate) fn pooled_list(&mut self, fill: impl FnOnce(&mut Vec<i32>)) -> Tokens {
        let mut t = self.tok_pool.pop().unwrap_or_default();
        if let Some(l) = Arc::get_mut(&mut t) {
            let b = l.buffer();
            b.clear();
            fill(b);
            l.remake(false);
            return t;
        }
        let mut v = Vec::new();
        fill(&mut v);
        self.make_list(v)
    }

    /// A list the input or a table let go of: back to the pool if no one
    /// else holds it.
    #[inline]
    pub(crate) fn release_list(&mut self, t: Tokens) {
        if Arc::strong_count(&t) == 1
            && self.tok_pool.len() < POOL_MAX
            && POOL.load(core::sync::atomic::Ordering::Relaxed)
        {
            self.tok_pool.push(t);
        }
    }

    /// `def_ref`, the list a definition built (§473), made a value; the
    /// builder is left empty.
    pub(crate) fn take_def(&mut self) -> Tokens {
        // (a copy of its size: the builder keeps its room for the next)
        let toks = self.def_ref.clone();
        self.def_ref.clear();
        let protected = core::mem::take(&mut self.def_protected);
        Arc::new(TokenList::new(toks, protected))
    }

    /// §162: `omit_template`, `\endtemplate` alone (made once).
    pub(crate) fn omit_template(&self) -> Tokens {
        self.omit_list.clone()
    }

    /// §162: `null_list`, the empty list (made once).
    pub(crate) fn null_list(&self) -> Tokens {
        self.empty_list.clone()
    }

    /// The constant lists made again, versioned (a build that records
    /// switches versions on after the engine is made).
    pub(crate) fn remake_constant_lists(&mut self) {
        self.empty_list = TokenList::shared(&[]);
        self.omit_list = TokenList::shared(&[END_TEMPLATE_TOKEN]);
    }

    /// The token at `loc` of the current level's list and the `loc` after
    /// it (`NULL` past the end).
    #[inline(always)]
    #[allow(clippy::inline_always, reason = "the token path")]
    pub(crate) fn cur_tok_and_next(&self) -> (i32, i32) {
        let l = self
            .cur_input
            .list
            .as_deref()
            .map_or(&[][..], TokenList::tokens);
        let i = crate::input::ux(self.cur_input.loc);
        let t = l[i];
        let next = if i + 1 < l.len() {
            i32::try_from(i + 1).unwrap_or(crate::mem::NULL)
        } else {
            crate::mem::NULL
        };
        (t, next)
    }

    /// The token at `loc` of the current level's list.
    #[inline]
    pub(crate) fn cur_tok_at(&self, loc: i32) -> i32 {
        let l = self
            .cur_input
            .list
            .as_deref()
            .map_or(&[][..], TokenList::tokens);
        l[crate::input::ux(loc)]
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §323 for the list eqtb location `loc` holds (`\everypar` and the
    /// like), if there is one: the value itself, shared.
    pub(crate) fn begin_toks_at(&mut self, loc: i32, t: i32) -> Result<(), crate::tex::Jump> {
        match self.equiv_toks(loc).cloned() {
            Some(l) => self.begin_token_list(l, t),
            None => Ok(()),
        }
    }

    /// e-TeX's joined `\aftergroup` (§326): token `t` put in front of the
    /// backed-up list the current level reads, a new list (a value is not
    /// changed in place; the pooled one is reused when no one else holds
    /// it).
    pub(crate) fn insert_front(&mut self, t: i32) {
        let old = self.cur_input.list.take().unwrap_or_default();
        let new = self.pooled_list(|b| {
            b.push(t);
            b.extend_from_slice(old.tokens());
        });
        self.release_list(old);
        self.cur_input.list = Some(new);
        self.cur_input.loc = 0;
    }
}

/// §323: the `loc` of a list's first token (`NULL` for an empty list).
#[inline]
#[must_use]
pub(crate) fn list_start(p: &TokenList) -> i32 {
    if p.is_empty() { crate::mem::NULL } else { 0 }
}
