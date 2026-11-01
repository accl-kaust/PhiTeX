//! Graphite smart-font shaping: a Rust port of graphite2 1.3.15, the
//! version XeTeX runs with in TeX Live 2026 (see NOTICE).
//!
//! `no_std` + `alloc`, no I/O: a [`Face`] is made from its tables (as
//! `gr_make_face_with_ops(..., gr_face_preloadAll)`), a [`Segment`] from
//! its characters and [`Features`] (as `gr_make_seg`); the segment's
//! slots are read as the C API reads them (`gr_slot_*`).
//!
//! The arithmetic is graphite2's: `float` where it computes in `float`,
//! in the same order, so positions are bit for bit the library's.
//!
//! Modules follow graphite2's files: [`face`] (`Face.cpp`,
//! `gr_face.cpp`), `glyph_cache`, `cmap`, `feature_map`, `name_table`,
//! `silf`, `pass`, `code` and `machine` (the rule programs), `rule`,
//! `segment` (`Segment.cpp`, `Slot.cpp`), `collider` and `intervals`
//! (collision avoidance and kerning), `justifier`.

#![no_std]
// The port keeps graphite2's integer and float conversions (`int(x)`
// truncations, `uint16` wraps): they are what the C++ computes.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    clippy::float_cmp,
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::unreadable_literal,
    clippy::too_many_lines,
    clippy::struct_excessive_bools,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::needless_range_loop,
    clippy::if_not_else,
    clippy::bool_to_int_with_if,
    // (graphite2's `0.5f * (a + b)` is not `midpoint`: kept as written)
    clippy::manual_midpoint,
    // (graphite2's ISQRT2 literal, as written)
    clippy::approx_constant,
    clippy::excessive_precision,
    clippy::match_same_arms,
    clippy::collapsible_match,
    clippy::unused_self,
    clippy::trivially_copy_pass_by_ref,
    clippy::struct_field_names,
    clippy::single_match_else,
    clippy::too_many_arguments,
    clippy::manual_range_contains,
    clippy::needless_late_init,
    clippy::assign_op_pattern
)]

extern crate alloc;

mod be;
mod cmap;
mod code;
mod collider;
pub mod face;
mod feature_map;
pub mod font;
mod glyph_cache;
mod intervals;
mod justifier;
mod lz4;
mod machine;
mod name_table;
mod pass;
mod position;
mod rule;
mod segment;
mod silf;
mod sparse;
mod ttf;

pub use face::Face;
pub use feature_map::{FeatureRef, Features};
pub use font::Font;
pub use justifier::{JUST_END_INLINE, JUST_START_INLINE};
pub use position::{Position, Rect};
pub use segment::{NIL, Segment};
pub use silf::FaceInfo;

/// `gr_engine_version`.
pub const VERSION: (u32, u32, u32) = (1, 3, 15);

/// `gr_char_info` of a segment's character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CharInfo {
    pub unicode_char: u32,
    pub break_weight: i32,
    pub after: i32,
    pub before: i32,
    pub base: usize,
}

/// The C API's segment and slot functions (`gr_seg_*`, `gr_slot_*`). A
/// slot is a `u32`, [`NIL`] for none.
impl Segment<'_> {
    /// `gr_seg_advance_X`.
    #[must_use]
    pub fn advance_x(&self) -> f32 {
        self.advance.x
    }

    /// `gr_seg_advance_Y`.
    #[must_use]
    pub fn advance_y(&self) -> f32 {
        self.advance.y
    }

    /// `gr_seg_n_cinfo`.
    #[must_use]
    pub fn n_cinfo(&self) -> usize {
        self.charinfo.len()
    }

    /// `gr_seg_cinfo`.
    #[must_use]
    pub fn cinfo(&self, i: usize) -> Option<CharInfo> {
        self.charinfo.get(i).map(|c| CharInfo {
            unicode_char: c.char as u32,
            break_weight: i32::from(c.break_weight),
            after: c.after,
            before: c.before,
            base: c.base,
        })
    }

    /// `gr_seg_n_slots`.
    #[must_use]
    pub fn n_slots(&self) -> usize {
        self.slot_count()
    }

    /// `gr_seg_first_slot`.
    #[must_use]
    pub fn first_slot(&self) -> u32 {
        self.first
    }

    /// `gr_seg_last_slot`.
    #[must_use]
    pub fn last_slot(&self) -> u32 {
        self.last
    }

    /// `gr_slot_next_in_segment`.
    #[must_use]
    pub fn slot_next(&self, s: u32) -> u32 {
        self.s(s).next
    }

    /// `gr_slot_prev_in_segment`.
    #[must_use]
    pub fn slot_prev(&self, s: u32) -> u32 {
        self.s(s).prev
    }

    /// `gr_slot_attached_to`.
    #[must_use]
    pub fn slot_attached_to(&self, s: u32) -> u32 {
        self.s(s).parent
    }

    /// `gr_slot_first_attachment`.
    #[must_use]
    pub fn slot_first_attachment(&self, s: u32) -> u32 {
        self.s(s).child
    }

    /// `gr_slot_next_sibling_attachment`.
    #[must_use]
    pub fn slot_next_sibling_attachment(&self, s: u32) -> u32 {
        self.s(s).sibling
    }

    /// `gr_slot_gid`: the glyph (a pseudo glyph's real one).
    #[must_use]
    pub fn slot_gid(&self, s: u32) -> u16 {
        self.s(s).glyph()
    }

    /// `gr_slot_origin_X`.
    #[must_use]
    pub fn slot_origin_x(&self, s: u32) -> f32 {
        self.s(s).position.x
    }

    /// `gr_slot_origin_Y`.
    #[must_use]
    pub fn slot_origin_y(&self, s: u32) -> f32 {
        self.s(s).position.y
    }

    /// `gr_slot_advance_X(slot, face, font)`.
    #[must_use]
    pub fn slot_advance_x(&self, s: u32, font: Option<&Font>) -> f32 {
        let sl = self.s(s);
        let res = sl.advance.x;
        match font {
            Some(f) => {
                let gid = sl.glyph();
                if f.is_hinted() && gid < self.face.glyphs.num_glyphs() {
                    (res - self.face.glyphs.glyph(gid).advance.x) * f.scale + f.advance(gid)
                } else {
                    res * f.scale
                }
            }
            None => res,
        }
    }

    /// `gr_slot_advance_Y(slot, face, font)`.
    #[must_use]
    pub fn slot_advance_y(&self, s: u32, font: Option<&Font>) -> f32 {
        let res = self.s(s).advance.y;
        font.map_or(res, |f| res * f.scale)
    }

    /// `gr_slot_before`.
    #[must_use]
    pub fn slot_before(&self, s: u32) -> i32 {
        self.s(s).before
    }

    /// `gr_slot_after`.
    #[must_use]
    pub fn slot_after(&self, s: u32) -> i32 {
        self.s(s).after
    }

    /// `gr_slot_index`.
    #[must_use]
    pub fn slot_index(&self, s: u32) -> u32 {
        self.s(s).index
    }

    /// `gr_slot_attr(slot, seg, index, subindex)`.
    #[must_use]
    pub fn slot_attr(&self, s: u32, index: u8, sub: u8) -> i32 {
        self.get_attr(s, index, sub)
    }

    /// `gr_slot_can_insert_before`.
    #[must_use]
    pub fn slot_can_insert_before(&self, s: u32) -> bool {
        self.s(s).is_insert_before()
    }

    /// `gr_slot_original`.
    #[must_use]
    pub fn slot_original(&self, s: u32) -> i32 {
        self.s(s).original as i32
    }

    /// The slots in order.
    pub fn slots(&self) -> impl Iterator<Item = u32> + '_ {
        let mut s = self.first;
        core::iter::from_fn(move || {
            if s == NIL {
                return None;
            }
            let r = s;
            s = self.s(s).next;
            Some(r)
        })
    }
}
