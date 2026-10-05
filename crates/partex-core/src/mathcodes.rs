//! `XeTeX`'s math codes (`xetex.h`): a 32-bit word with the family in its
//! top 8 bits, the class in the next 3 and the character in the low 21
//! (`\Umathcode`). The word is kept in an `i32`, its bits unsigned: a
//! family past 127 makes it negative.

/// Class 7: use the current family (`var_fam_class`).
pub(crate) const VAR_FAM_CLASS: i32 = 7;
/// The character of an active math character (`active_math_char`).
pub(crate) const ACTIVE_MATH_CHAR: i32 = 0x1F_FFFF;

#[allow(clippy::cast_sign_loss, reason = "the word's bits")]
#[inline]
fn bits(x: i32) -> u32 {
    x as u32
}

#[allow(clippy::cast_possible_wrap, reason = "the word's bits")]
#[inline]
fn word(x: u32) -> i32 {
    x as i32
}

/// `math_fam_field`.
pub(crate) fn math_fam_field(x: i32) -> i32 {
    word((bits(x) >> 24) & 0xFF)
}

/// `math_class_field`.
pub(crate) fn math_class_field(x: i32) -> i32 {
    word((bits(x) >> 21) & 0x07)
}

/// `math_char_field`.
pub(crate) fn math_char_field(x: i32) -> i32 {
    word(bits(x) & 0x1F_FFFF)
}

/// `set_family_field`.
pub(crate) fn set_family_field(x: i32) -> i32 {
    word((bits(x) & 0xFF) << 24)
}

/// `set_class_field`.
pub(crate) fn set_class_field(x: i32) -> i32 {
    word((bits(x) & 0x07) << 21)
}

/// A math code from its class, family and character (the sum `XeTeX`
/// writes, its parts disjoint).
pub(crate) fn math_code_of(class: i32, fam: i32, c: i32) -> i32 {
    word(bits(set_class_field(class)) | bits(set_family_field(fam)) | (bits(c) & 0x1F_FFFF))
}

/// `is_active_math_char`.
pub(crate) fn is_active_math_char(x: i32) -> bool {
    math_char_field(x) == ACTIVE_MATH_CHAR
}

/// `is_var_family`.
pub(crate) fn is_var_family(x: i32) -> bool {
    math_class_field(x) == VAR_FAM_CLASS
}
