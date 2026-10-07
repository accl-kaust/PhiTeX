//! Relocatable numbers (DESIGN 4.1, "Relocatable values").
//!
//! Some counters only number things: LaTeX's mark ids (`\g__mark_int`),
//! which every `\marks` text carries and the output routine copies and
//! compares. An edit that adds a mark shifts every later id, so every
//! later page's state differs from the old run's although no page shows
//! an id. A value is *relocatable* where the job only copies it, compares
//! it for equality, or adds a constant to it; anything else that depends
//! on the number itself (typesetting it, writing it to a file, comparing
//! it with a constant, arithmetic) *observes* it. A rebuild may then reuse
//! a region whose entry differs from its old run's only by a shift of
//! such numbers, and that observed none of them, with the numbers it made
//! shifted the same way (the machine and the runtime, `machine.rs` and
//! `partex-incr`).
//!
//! What is relocatable is decided from the uses, never from names:
//! - a count register's number is an *origin*. Reading the register
//!   through `scan_something_internal` gives `cur_val` that origin
//!   ([`Tex::cur_val_origin`]) if the caller asked for it: `\the` and
//!   `\number` then make *tagged* digits (`partex_engine::web::TAG_BASE`),
//!   `\ifnum` records its answer ([`Tracker::int_answer`]), `\numexpr`
//!   keeps the origin of a sum with constants; `\advance` by a constant is
//!   affine. Any other read, and any other assignment, observes the
//!   origin ([`Tracker::observe`]);
//! - a tagged digit reads as its digit (`tok_chr`), so TeX behaves as it
//!   would with plain digits. `get_next` keeps the token as it is in its
//!   list ([`Tex::cur_raw`]); the places that copy tokens (a macro's
//!   arguments, a body or text scanned, an alignment's preamble, a token
//!   backed up) store it as it is ([`Tex::take_raw_tok`]), and any other
//!   use observes its origin: the tag stays pending until the next token
//!   is read or the region ends ([`Tex::flush_tag`]). The places that
//!   read lists without `get_next` observe themselves: showing a list
//!   (printing, `\write`, `\message`, `\meaning`, `\detokenize`, …),
//!   `\ifx` on macros (an equality that holds for both runs only between
//!   numbers of one origin), a macro's digit delimiters, `\uppercase` of a
//!   digit with a case code; runs taken in bulk stop at a tagged token.
//!
//! Tags are made only with [`Tex::tags_on`] (the machine's switch).

use alloc::vec::Vec;

use crate::host::Host;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::{
    COUNT_BASE, INT_VAL, OTHER_TOKEN, TAG_ORIGINS, is_tagged, tag_of, tagged_digit, untag,
};

/// No origin (`Tex::cur_val_origin`).
pub(crate) const NO_ORIGIN: i32 = -1;

/// The relation of an answer ([`Tracker::int_answer`]) that says a
/// number `x` had `y` added: it holds where `x` and `x + y` move alike.
pub const SHIFT: u8 = b'+';

/// Whether an answer ([`Tracker::int_answer`]) about number `x` of a
/// moving origin still holds with `x` relocated by `s`: `rel` is `<`,
/// `=` or `>` (`x rel y` was `answer`) or [`SHIFT`].
#[must_use]
pub fn answer_holds(s: &Shifts, o: i32, x: i32, rel: u8, y: i32, answer: bool) -> bool {
    let (x, y) = (i64::from(x), i64::from(y));
    let nx = s.map(o, x);
    match rel {
        SHIFT => nx + y == s.map(o, x + y),
        b'<' => (nx < y) == answer,
        b'>' => (nx > y) == answer,
        _ => (nx == y) == answer,
    }
}

/// The origin of eqtb location `loc` if it is a count register's (its
/// number), else `None`.
#[must_use]
pub fn origin_of_loc(loc: i32) -> Option<i32> {
    if (COUNT_BASE..COUNT_BASE + 256).contains(&loc) {
        return Some(loc - COUNT_BASE);
    }
    if loc >= crate::xregs::EXT_BASE && crate::wide::wide_of(loc).is_none() {
        let (kind, n) = crate::xregs::ext_reg(loc);
        if kind == INT_VAL {
            return Some(n);
        }
    }
    None
}

/// The eqtb location of origin `o` (count register `o`).
#[must_use]
pub fn loc_of_origin(o: i32) -> i32 {
    crate::xregs::reg_loc(INT_VAL, o)
}

/// The tagged digits of `v` (not negative) of origin `o`.
pub(crate) fn tag_number(o: i32, v: i32) -> Vec<i32> {
    let digits = alloc::format!("{v}");
    digits
        .bytes()
        .enumerate()
        .map(|(i, d)| tagged_digit(o, i == 0, i32::from(d - b'0')))
        .collect()
}

/// A relocation: for each origin, numbers above `base` move by `delta`
/// (`partex_incr::Reloc` as the engine takes it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shifts(pub Vec<(i32, i64, i64)>);

impl Shifts {
    /// Number `x` of origin `o`, relocated.
    #[must_use]
    pub fn map(&self, o: i32, x: i64) -> i64 {
        match self.0.iter().find(|s| s.0 == o) {
            Some(&(_, base, delta)) if x > base => x + delta,
            _ => x,
        }
    }

    /// Whether origin `o` moves.
    #[must_use]
    pub fn moves(&self, o: i32) -> bool {
        self.0.iter().any(|s| s.0 == o && s.2 != 0)
    }
}

/// `toks` with every tagged number of a moving origin relocated, if any
/// is (`None`: nothing changes). A tagged digit that does not continue a
/// number begun with a first digit of its origin is a number split by a
/// partial copy: it cannot be relocated, and `Err` says so.
#[allow(
    clippy::result_unit_err,
    reason = "a split number: nothing more to say"
)]
pub fn shift_tokens(toks: &[i32], s: &Shifts) -> Result<Option<Vec<i32>>, ()> {
    if !toks.iter().any(|&t| is_tagged(t)) {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(toks.len());
    let mut changed = false;
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        let Some((o, first, d)) = tag_of(t) else {
            out.push(t);
            i += 1;
            continue;
        };
        if !s.moves(o) {
            out.push(t);
            i += 1;
            continue;
        }
        if !first {
            return Err(());
        }
        let mut v = i64::from(d);
        let mut j = i + 1;
        while let Some((o2, false, d2)) = toks.get(j).and_then(|&u| tag_of(u)) {
            if o2 != o {
                break;
            }
            v = v * 10 + i64::from(d2);
            j += 1;
        }
        let w = s.map(o, v);
        if w != v {
            changed = true;
        }
        let w = i32::try_from(w).map_err(|_| ())?;
        if w < 0 {
            return Err(());
        }
        out.extend(tag_number(o, w));
        i = j;
    }
    Ok(changed.then_some(out))
}

/// The largest number of origin `o` among the tagged digits of `toks`.
#[must_use]
pub fn max_tag(toks: &[i32], o: i32) -> Option<i64> {
    let mut best = None;
    let mut i = 0;
    while i < toks.len() {
        match tag_of(toks[i]) {
            Some((o1, true, d)) if o1 == o => {
                let mut v = i64::from(d);
                let mut j = i + 1;
                while let Some((o2, false, d2)) = toks.get(j).and_then(|&u| tag_of(u)) {
                    if o2 != o {
                        break;
                    }
                    v = v * 10 + i64::from(d2);
                    j += 1;
                }
                best = best.max(Some(v));
                i = j;
            }
            // (a digit of a split number: unknown, the largest possible)
            Some((o1, false, _)) if o1 == o => return Some(i64::MAX),
            _ => i += 1,
        }
    }
    best
}

/// Whether two lists are equal as TeX compares them (tagged digits as
/// their digits; `\ifx` on macros, §508), and the origins the answer
/// depends on: those of tagged digits that face a plain digit or a digit
/// of another origin (none: it holds for both runs of a relocation).
pub(crate) fn tagged_eq(a: &[i32], b: &[i32]) -> (bool, Vec<i32>) {
    let eq = a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| untag(x) == untag(y));
    let mut seen = Vec::new();
    if a.iter().chain(b).any(|&t| is_tagged(t)) {
        let n = a.len().max(b.len());
        for k in 0..n {
            let (x, y) = (a.get(k).copied(), b.get(k).copied());
            let (tx, ty) = (x.and_then(tag_of), y.and_then(tag_of));
            match (tx, ty) {
                (Some((o1, ..)), Some((o2, ..))) if o1 == o2 => {}
                (Some((o, ..)), _) | (_, Some((o, ..))) => {
                    // (facing a plain digit or another origin's: the
                    // number decides; facing anything else, it does not)
                    let other = if tx.is_some() { y } else { x };
                    let digit = other.is_some_and(|u| {
                        (OTHER_TOKEN + i32::from(b'0')..=OTHER_TOKEN + i32::from(b'9'))
                            .contains(&untag(u))
                    });
                    if digit && !seen.contains(&o) {
                        seen.push(o);
                    }
                }
                _ => {}
            }
        }
    }
    (eq, seen)
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Turn tagged digits on or off (the machine's switch).
    pub fn set_tags(&mut self, on: bool) {
        // (never in a format: INITEX's macros hold plain digits)
        self.tags_on = on && !self.memo.enabled && !self.params.ini;
    }

    /// The job observed a value of origin `o`.
    #[inline]
    pub(crate) fn observe_origin(&self, o: i32) {
        if self.tags_on {
            self.tracker.observe(o);
        }
    }

    /// A tagged token read and not stored or backed up: observed.
    #[cold]
    #[inline(never)]
    pub(crate) fn flush_tag(&mut self) {
        let p = core::mem::take(&mut self.tag_pending);
        if p != 0 {
            self.tracker.observe(p - 1);
        }
    }

    /// Note a tagged token `t` that `get_next` just read.
    #[cold]
    #[inline(never)]
    pub(crate) fn read_tagged(&mut self, t: i32) {
        if self.tag_pending != 0 {
            self.flush_tag();
        }
        self.cur_raw = t;
        if let Some((o, ..)) = tag_of(t) {
            self.tag_pending = o + 1;
        }
    }

    /// `cur_tok` as it was in its list, to store or back up: a copy, no
    /// observation.
    #[inline]
    pub(crate) fn take_raw_tok(&mut self) -> i32 {
        if self.cur_raw != 0 && untag(self.cur_raw) == self.cur_tok {
            self.tag_pending = 0;
            core::mem::take(&mut self.cur_raw)
        } else {
            self.cur_tok
        }
    }

    /// The tokens `\the` or `\number` make of `cur_val`, which has origin
    /// `o`: tagged digits if it can be tagged, else plain ones (and `o`
    /// observed).
    pub(crate) fn origin_digits(&self, o: i32, v: i32) -> Option<Vec<i32>> {
        if o == NO_ORIGIN {
            return None;
        }
        if v >= 0 && o < TAG_ORIGINS {
            Some(tag_number(o, v))
        } else {
            self.observe_origin(o);
            None
        }
    }

    /// §440 for a caller that takes an origin: `cur_val`, and its origin
    /// if it is a count register read as it is (`NO_ORIGIN` else).
    pub(crate) fn scan_int_origin(&mut self) -> Result<i32, Jump> {
        if !self.tags_on {
            self.scan_int()?;
            return Ok(NO_ORIGIN);
        }
        self.int_origin_req = true;
        self.scan_int()?;
        Ok(core::mem::replace(&mut self.cur_val_origin, NO_ORIGIN))
    }

    /// `cur_val` has origin `o` (or none) at the end of
    /// `scan_something_internal`: kept for a caller that asked (`want`),
    /// observed else.
    pub(crate) fn settle_origin(&mut self, o: i32, want: bool) {
        if o == NO_ORIGIN {
            return;
        }
        if want {
            self.cur_val_origin = o;
        } else {
            self.observe_origin(o);
        }
    }

    /// `\ifnum` compared `x` (origin `ox`) with `y` (origin `oy`), relation
    /// `rel`, answer `answer`: with one origin and a constant, the answer
    /// is what the job depends on; two numbers of one origin compare
    /// alike in both runs of a relocation; two origins are observed.
    pub(crate) fn note_int_answer(&self, ox: i32, x: i32, rel: u8, oy: i32, y: i32, answer: bool) {
        match (ox, oy) {
            (NO_ORIGIN, NO_ORIGIN) => {}
            (o, NO_ORIGIN) if self.tags_on => self.tracker.int_answer(o, x, rel, y, answer),
            (NO_ORIGIN, o) if self.tags_on => {
                let flipped = match rel {
                    b'<' => b'>',
                    b'>' => b'<',
                    r => r,
                };
                self.tracker.int_answer(o, y, flipped, x, answer);
            }
            (a, b) if a == b => {}
            (a, b) => {
                self.observe_origin(a);
                self.observe_origin(b);
            }
        }
    }

    /// A digit of a number `scan_int` reads (`first`: its first), for a
    /// caller that takes the number's origin: whether the number is still
    /// one origin's tagged digits, each in its place (`origin` the
    /// origin, `NO_ORIGIN` until the first). Such a digit is not observed;
    /// once the number is not, its origin is.
    pub(crate) fn track_tagged_digit(&mut self, origin: &mut i32, first: bool) -> bool {
        let tag = (untag(self.cur_raw) == self.cur_tok)
            .then(|| tag_of(self.cur_raw))
            .flatten();
        match tag {
            Some((o, f, _)) if f == first && (first || o == *origin) => {
                *origin = o;
                self.tag_pending = 0;
                true
            }
            _ => {
                if *origin != NO_ORIGIN {
                    self.observe_origin(*origin);
                    *origin = NO_ORIGIN;
                }
                false
            }
        }
    }

    /// Count register `loc`, holding `x`, is advanced by the constant
    /// `c`: its origin's numbers stay relocatable where `x` and `x + c`
    /// move alike, an answer the job depends on.
    pub(crate) fn note_count_shift(&self, loc: i32, x: i32, c: i32) {
        if self.tags_on
            && let Some(o) = origin_of_loc(loc)
        {
            self.note_shift(o, x, c);
        }
    }

    /// A number `x` of origin `o` (not `NO_ORIGIN`) had the constant `c`
    /// added, keeping its origin.
    pub(crate) fn note_shift(&self, o: i32, x: i32, c: i32) {
        if self.tags_on && c != 0 {
            self.tracker.int_answer(o, x, SHIFT, c, true);
        }
    }

    /// An assignment of `loc`: an observation of its origin unless it is
    /// `\advance` by a constant ([`Tex::affine_count`]).
    #[inline]
    pub(crate) fn note_count_write(&mut self, loc: i32) {
        if !self.tags_on {
            return;
        }
        if self.affine_count == loc {
            self.affine_count = 0;
            return;
        }
        if let Some(o) = origin_of_loc(loc) {
            self.observe_origin(o);
        }
    }

    /// A read of `loc` that uses the number: an observation of its origin.
    #[inline]
    pub(crate) fn note_count_read(&self, loc: i32) {
        if self.tags_on
            && let Some(o) = origin_of_loc(loc)
        {
            self.tracker.observe(o);
        }
    }
}
