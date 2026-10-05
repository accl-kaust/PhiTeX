//! `XeTeX`'s tables past character 255 (DESIGN 4.7): active characters,
//! single-character control sequences and the cat, lc, uc, sf, math and
//! del codes of every Unicode scalar value. Characters 0–255 keep their
//! places in eqtb; the others live at locations past it, held by the map
//! that holds e-TeX's registers above 255 (`xregs.rs`) with `IniTeX`'s
//! values as defaults, so that defining, saving, restoring and tracing
//! them is eqtb's code.

use crate::web::{
    ACTIVE_BASE, CAT_CODE_BASE, DEL_CODE_BASE, LC_CODE_BASE, MATH_CODE_BASE, NULL_CS, SF_CODE_BASE,
    SINGLE_BASE, UC_CODE_BASE,
};

/// `XeTeX`'s `biggest_char`: the largest UTF-16 unit (TeX's is 255).
pub(crate) const XETEX_BIGGEST_CHAR: i32 = 0xFFFF;

/// The first location of a wide table.
pub(crate) const WIDE_BASE: i32 = 0x3000_0000;
/// The locations of one table.
const SPAN: i32 = 0x20_0000;

/// The tables, in their order past [`WIDE_BASE`].
pub(crate) const ACTIVE: i32 = 0;
pub(crate) const SINGLE: i32 = 1;
pub(crate) const CAT: i32 = 2;
pub(crate) const LC: i32 = 3;
pub(crate) const UC: i32 = 4;
pub(crate) const SF: i32 = 5;
pub(crate) const MATH: i32 = 6;
pub(crate) const DEL: i32 = 7;

/// Location of character `c` (past 255) in table `t`.
#[inline]
pub(crate) fn wide_loc(t: i32, c: i32) -> i32 {
    WIDE_BASE + t * SPAN + c
}

/// The table and character of a wide location.
#[inline]
pub(crate) fn wide_of(p: i32) -> Option<(i32, i32)> {
    (p >= WIDE_BASE).then(|| ((p - WIDE_BASE) / SPAN, (p - WIDE_BASE) % SPAN))
}

/// The control sequence of active character `c` (`active_base+c`).
#[inline]
pub(crate) fn active_cs(c: i32) -> i32 {
    if c < 256 {
        ACTIVE_BASE + c
    } else {
        wide_loc(ACTIVE, c)
    }
}

/// The control sequence of the one-character name `c` (`single_base+c`).
#[inline]
pub(crate) fn single_cs(c: i32) -> i32 {
    if c < 256 {
        SINGLE_BASE + c
    } else {
        wide_loc(SINGLE, c)
    }
}

/// The character of an active character's control sequence.
#[inline]
pub(crate) fn active_char(p: i32) -> Option<i32> {
    if (ACTIVE_BASE..SINGLE_BASE).contains(&p) {
        return Some(p - ACTIVE_BASE);
    }
    match wide_of(p) {
        Some((ACTIVE, c)) => Some(c),
        _ => None,
    }
}

/// The character of a one-character control sequence's name.
#[inline]
pub(crate) fn single_char(p: i32) -> Option<i32> {
    if (SINGLE_BASE..NULL_CS).contains(&p) {
        return Some(p - SINGLE_BASE);
    }
    match wide_of(p) {
        Some((SINGLE, c)) => Some(c),
        _ => None,
    }
}

/// Whether `p` is a control sequence of one character, active or named
/// (TeX's `p<hash_base`, `null_cs` apart).
#[inline]
pub(crate) fn is_char_cs(p: i32) -> bool {
    p < NULL_CS || matches!(wide_of(p), Some((ACTIVE | SINGLE, _)))
}

/// The location of code `c` of the table at `base` (`cat_code_base`,
/// `lc_code_base`, …, `del_code_base`: `base+c` for `c<256`).
#[inline]
pub(crate) fn code_loc(base: i32, c: i32) -> i32 {
    if c < 256 {
        return base + c;
    }
    let t = match base {
        CAT_CODE_BASE => CAT,
        LC_CODE_BASE => LC,
        UC_CODE_BASE => UC,
        SF_CODE_BASE => SF,
        MATH_CODE_BASE => MATH,
        DEL_CODE_BASE => DEL,
        // (a table that has no wide part: as TeX would index it)
        _ => return base + c,
    };
    wide_loc(t, c)
}

/// The base and character of a code location (`code_loc` undone).
pub(crate) fn code_of(p: i32) -> Option<(i32, i32)> {
    let (t, c) = wide_of(p)?;
    let base = match t {
        CAT => CAT_CODE_BASE,
        LC => LC_CODE_BASE,
        UC => UC_CODE_BASE,
        SF => SF_CODE_BASE,
        MATH => MATH_CODE_BASE,
        DEL => DEL_CODE_BASE,
        _ => return None,
    };
    Some((base, c))
}

/// `IniTeX`'s value of code `c` of wide table `t` (`XeTeX` §232: other
/// characters, `math_code(k)=k`, `sf_code(k)=1000`, `del_code(k)=-1`).
pub(crate) fn initial_code(t: i32, c: i32) -> i32 {
    match t {
        CAT => crate::web::OTHER_CHAR,
        SF => 1000,
        MATH => c,
        DEL => -1,
        _ => 0,
    }
}
