//! Runs of tokens taken from the token list being read at once
//! (DESIGN.md §7.13; `Params::fast` bit `FAST_BULK`).
//!
//! Absorbing a macro's arguments (§392, §399), a definition's or token
//! list's body (§473, §477), and the characters of `\csname` (§372) reads
//! one `get_token` at a time and stores each token. Where the input is a
//! token list, a run of tokens that asks nothing of `get_next` (no macro
//! parameter, `\outer` or `\noexpand`ed control sequence, or alignment
//! entry's end) and that the caller would only store is copied from the
//! list in one go; the caller goes on from where the run stopped, with
//! the state it would have had after reading the run token by token: the
//! input position (`NULL` at the list's end, which the next `get_next`
//! leaves as it would), `align_state`, and its own counts. The token that
//! stops a run is read by the caller as before, so errors, expansion and
//! every rarer case take tex.web's path. The meanings a token-by-token
//! read looks up are looked up (`eqtb`), so trackers see the same reads;
//! with memo recording on, nothing is taken in bulk.

use crate::host::Host;
use crate::mem::NULL;
use crate::tex::Tex;
use crate::track::Tracker;
use crate::web::*;

/// How the run's caller treats a control sequence token.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Absorb {
    /// A macro's argument: any meaning is stored (`\par` ends a short
    /// macro's argument: `par_ends`).
    Arg { par_ends: bool },
    /// A body without expansion: a meaning of `mac_param` is special in a
    /// definition (`macro_def`).
    Body { macro_def: bool },
    /// A body with expansion (`\edef`, `\xdef`, `\message`, …): stored
    /// only if it does not expand (or is `\protected`).
    Expand { macro_def: bool },
}

/// Where a run of tokens goes: the argument being scanned
/// (`Tex::arg_list`) or the list a definition builds (`Tex::def_ref`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dst {
    Arg,
    Def,
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Whether runs are taken at all.
    #[inline(always)]
    #[allow(clippy::inline_always, reason = "a test of one bit, on the token path")]
    pub(crate) fn bulk_on(&self) -> bool {
        self.params.fast & crate::params::FAST_BULK != 0
    }

    /// Whether a run can be taken from the current input.
    #[inline(always)]
    #[allow(clippy::inline_always, reason = "a few tests, on the token path")]
    pub(crate) fn bulk_ready(&self) -> bool {
        self.bulk_on()
            && self.cur_input.state == TOKEN_LIST
            && self.cur_input.loc != NULL
            && !self.memo.recording()
            // (with glyph origins, a list whose tokens carry them, an
            // argument's, is read a token at a time: `srcmap.rs`)
            && (self.org.is_none() || self.cur_input.list.as_deref().is_none_or(|l| l.org() == 0))
    }

    /// Whether control sequence token `t` is stored as it is read, as
    /// `how` absorbs it, with `align` as `align_state`.
    #[inline(always)]
    #[allow(clippy::inline_always, reason = "measured: 1% of the course as a call")]
    fn bulk_cs(&self, t: i32, how: Absorb, align: i32) -> bool {
        // (an argument's or a body's token is only stored: its class is
        // what the run depends on, as for `get_next`'s lookup)
        let w = if matches!(how, Absorb::Expand { .. }) {
            self.eqtb(t - CS_TOKEN_FLAG)
        } else {
            self.token_meaning(t - CS_TOKEN_FLAG)
        };
        let cmd = w.b0();
        if cmd >= OUTER_CALL || (align == 0 && (cmd == TAB_MARK || cmd == CAR_RET)) {
            return false;
        }
        match how {
            Absorb::Arg { par_ends } => !(par_ends && t == self.par_token),
            Absorb::Body { macro_def } => !(macro_def && cmd == MAC_PARAM),
            Absorb::Expand { macro_def } => {
                if cmd <= MAX_COMMAND {
                    !(macro_def && cmd == MAC_PARAM)
                } else {
                    cmd >= CALL && self.is_protected(t - CS_TOKEN_FLAG)
                }
            }
        }
    }

    /// A run of a group's inside (§399, §477): tokens stored into list
    /// `dst` while `unbalance` (the braces open) stays positive; the `}`
    /// that closes the group is left to the caller. Returns the tokens
    /// taken.
    #[inline(always)]
    #[allow(
        clippy::inline_always,
        reason = "measured: the check inline, the run not"
    )]
    pub(crate) fn bulk_group(&mut self, dst: Dst, unbalance: &mut i32, how: Absorb) -> usize {
        if self.bulk_ready() {
            self.bulk_group_run(dst, unbalance, how)
        } else {
            0
        }
    }

    #[inline(never)]
    fn bulk_group_run(&mut self, dst: Dst, unbalance: &mut i32, how: Absorb) -> usize {
        // (the list read in place: no reference counted)
        let loc = crate::input::ux(self.cur_input.loc);
        let toks: &[i32] = self.cur_input.list.as_deref().map_or(&[], |l| l.tokens());
        let n = toks.len();
        let mut align = self.align_state();
        let mut u = *unbalance;
        let macro_def = matches!(
            how,
            Absorb::Body { macro_def: true } | Absorb::Expand { macro_def: true }
        );
        let mut k = loc;
        while let Some(&t) = toks.get(k) {
            if t >= CS_TOKEN_FLAG {
                if !self.bulk_cs(t, how, align) {
                    break;
                }
            } else {
                match tok_cmd(t) {
                    LEFT_BRACE => {
                        u += 1;
                        align += 1;
                    }
                    RIGHT_BRACE => {
                        if u == 1 {
                            break;
                        }
                        u -= 1;
                        align -= 1;
                    }
                    OUT_PARAM => break,
                    TAB_MARK if align == 0 => break,
                    MAC_PARAM if macro_def => break,
                    _ => {}
                }
            }
            k += 1;
        }
        self.bulk_take(dst, loc, k, n);
        self.set_align_state(align);
        *unbalance = u;
        k - loc
    }

    /// A run of a delimited argument at brace level zero with no partial
    /// match of the delimiter (§392): tokens other than the delimiter's
    /// first `delim` and braces stored into list `dst`. Returns the tokens
    /// taken (each one more for §392's `m`).
    #[inline(always)]
    #[allow(
        clippy::inline_always,
        reason = "measured: the check inline, the run not"
    )]
    pub(crate) fn bulk_level0(&mut self, dst: Dst, delim: i32, par_ends: bool) -> usize {
        if self.bulk_ready() {
            self.bulk_level0_run(dst, delim, par_ends)
        } else {
            0
        }
    }

    #[inline(never)]
    fn bulk_level0_run(&mut self, dst: Dst, delim: i32, par_ends: bool) -> usize {
        // (the list read in place: no reference counted)
        let loc = crate::input::ux(self.cur_input.loc);
        let toks: &[i32] = self.cur_input.list.as_deref().map_or(&[], |l| l.tokens());
        let n = toks.len();
        let align = self.align_state();
        let how = Absorb::Arg { par_ends };
        let mut k = loc;
        while let Some(&t) = toks.get(k) {
            // (a tagged digit is compared as its digit, token by token:
            // `reloc.rs`)
            if t == delim || is_tagged(t) {
                break;
            }
            if t >= CS_TOKEN_FLAG {
                if !self.bulk_cs(t, how, align) {
                    break;
                }
            } else if matches!(tok_cmd(t), LEFT_BRACE | RIGHT_BRACE | OUT_PARAM)
                || (tok_cmd(t) == TAB_MARK && align == 0)
            {
                break;
            }
            k += 1;
        }
        self.bulk_take(dst, loc, k, n);
        k - loc
    }

    /// A run of character tokens of a `\csname` (§372), stored into
    /// `name`.
    #[inline(always)]
    #[allow(
        clippy::inline_always,
        reason = "measured: the check inline, the run not"
    )]
    pub(crate) fn bulk_chars(&mut self, name: &mut alloc::vec::Vec<i32>) {
        if self.bulk_ready() {
            self.bulk_chars_run(name);
        }
    }

    #[inline(never)]
    fn bulk_chars_run(&mut self, name: &mut alloc::vec::Vec<i32>) {
        // (the list read in place: no reference counted)
        let loc = crate::input::ux(self.cur_input.loc);
        let toks: &[i32] = self.cur_input.list.as_deref().map_or(&[], |l| l.tokens());
        let n = toks.len();
        let mut align = self.align_state();
        let mut k = loc;
        while let Some(&t) = toks.get(k) {
            match tok_cmd(t) {
                // (a name made of a tagged digit observes it, token by
                // token: `reloc.rs`)
                _ if t >= CS_TOKEN_FLAG || is_tagged(t) => break,
                LEFT_BRACE => align += 1,
                RIGHT_BRACE => align -= 1,
                OUT_PARAM => break,
                TAB_MARK if align == 0 => break,
                _ => {}
            }
            k += 1;
        }
        name.extend_from_slice(&toks[loc..k]);
        self.cur_input.loc = Self::loc_after(k, n);
        self.set_align_state(align);
    }

    /// Tokens `a..b` of the current list `src` stored into list `dst`,
    /// and read (the list has `n`).
    fn bulk_take(&mut self, dst: Dst, a: usize, b: usize, n: usize) {
        if b > a {
            let src = self
                .cur_input
                .list
                .as_deref()
                .map_or(&[][..], |l| l.tokens());
            match dst {
                Dst::Arg => self.arg_list.extend_from_slice(&src[a..b]),
                Dst::Def => self.def_ref.extend_from_slice(&src[a..b]),
            }
            self.cur_input.loc = Self::loc_after(b, n);
        }
    }

    /// The `loc` of index `k` of a list of `n` tokens.
    fn loc_after(k: usize, n: usize) -> i32 {
        if k < n {
            i32::try_from(k).unwrap_or(NULL)
        } else {
            NULL
        }
    }

    /// A run of skipped conditional text (§494): tokens passed over up to
    /// the next conditional, `\fi`, `\else` or `\or` (which `pass_text`
    /// reads and counts as before).
    #[inline(always)]
    #[allow(
        clippy::inline_always,
        reason = "measured: the check inline, the run not"
    )]
    pub(crate) fn bulk_skip(&mut self) {
        if self.bulk_ready() {
            self.bulk_skip_run();
        }
    }

    #[inline(never)]
    fn bulk_skip_run(&mut self) {
        // (the list read in place: no reference counted)
        let loc = crate::input::ux(self.cur_input.loc);
        let toks: &[i32] = self.cur_input.list.as_deref().map_or(&[], |l| l.tokens());
        let n = toks.len();
        let mut align = self.align_state();
        let mut k = loc;
        while let Some(&t) = toks.get(k) {
            if t >= CS_TOKEN_FLAG {
                let c = self.token_meaning(t - CS_TOKEN_FLAG).b0();
                if c == IF_TEST
                    || c == FI_OR_ELSE
                    || c >= OUTER_CALL
                    || (align == 0 && (c == TAB_MARK || c == CAR_RET))
                {
                    break;
                }
            } else {
                match tok_cmd(t) {
                    LEFT_BRACE => align += 1,
                    RIGHT_BRACE => align -= 1,
                    OUT_PARAM => break,
                    TAB_MARK if align == 0 => break,
                    _ => {}
                }
            }
            k += 1;
        }
        self.cur_input.loc = Self::loc_after(k, n);
        self.set_align_state(align);
    }
}
