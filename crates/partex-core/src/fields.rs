//! Field-level versions of node lists, for the pure SSA tracer
//! (`Tracker::PURE`, DESIGN 3.17): what the line breaker, the packers and
//! the page builder read of a list, apart from what shipping it out reads.
//!
//! A character is two values: its metrics (width, height, depth, italic
//! correction) and its identity (the code and the font, what the page's
//! content stream shows). The line breaker reads the metrics, and the
//! letters of a word it may hyphenate (a character whose `\lccode` is not
//! zero, with its font, whose `\hyphenchar` it reads); a pack reads the
//! metrics; a box is read by its dimensions, never its contents. So a
//! `\pageref` that goes from 184 to 185 changes the identity of one
//! character and none of these versions.
//!
//! Nothing here is read through the tracker: the tracer asks for these
//! versions in place of the list's own, and the engine's tracking is
//! unchanged.

use alloc::vec::Vec;

use partex_engine::node::{Disc, Node};
use partex_ssa::Version;

use crate::host::Host;
use crate::tex::Tex;
use crate::track::Tracker;

/// What a field version reads beside the metrics.
#[derive(Clone, Copy)]
pub(crate) struct Reads {
    /// The letters (`\lccode` not zero) and their fonts: a hyphenation
    /// pass's input.
    pub letters: bool,
    /// The marks' tokens: what the page builder keeps for the output
    /// routine.
    pub marks: bool,
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// The field version of `list` under `reads`.
    pub(crate) fn fields_version(&self, list: &[Node], reads: Reads) -> u128 {
        let mut v: Vec<i64> = Vec::with_capacity(list.len() * 4);
        self.fields_into(list, reads, &mut v);
        Version::of(&v).0
    }

    fn glyph_fields(&self, f: u16, c: u8, reads: Reads, v: &mut Vec<i64>) {
        let g = self
            .fonts
            .get(i32::from(f))
            .glyph(i32::from(c))
            .unwrap_or_default();
        v.extend([
            i64::from(g.width),
            i64::from(g.height),
            i64::from(g.depth),
            i64::from(g.italic),
        ]);
        if reads.letters && self.peek_code(crate::web::LC_CODE_BASE, i32::from(c)) != 0 {
            v.extend([-1, i64::from(f), i64::from(c)]);
        }
    }

    fn fields_into(&self, list: &[Node], reads: Reads, v: &mut Vec<i64>) {
        for n in list {
            match n {
                Node::Glyphs(g) => {
                    v.push(1);
                    for &c in g.chars() {
                        self.glyph_fields(g.font.0, c, reads, v);
                    }
                }
                Node::Ligature(l) => {
                    v.push(2);
                    self.glyph_fields(
                        l.font.0,
                        l.ch,
                        Reads {
                            letters: false,
                            ..reads
                        },
                        v,
                    );
                    if reads.letters {
                        for &c in &l.original {
                            if self.peek_code(crate::web::LC_CODE_BASE, i32::from(c)) != 0 {
                                v.extend([-1, i64::from(l.font.0), i64::from(c)]);
                            }
                        }
                    }
                }
                Node::Box(b) => v.extend([
                    3,
                    i64::from(b.vertical),
                    i64::from(b.width),
                    i64::from(b.height),
                    i64::from(b.depth),
                    i64::from(b.shift),
                ]),
                Node::Rule {
                    width,
                    height,
                    depth,
                    ..
                } => v.extend([4, i64::from(*width), i64::from(*height), i64::from(*depth)]),
                Node::Glue { spec, subtype, .. } => {
                    v.extend([5, i64::from(*subtype)]);
                    v.push(i64::try_from(Version::of(spec).0 >> 66).unwrap_or(0));
                }
                Node::Leaders(l) => {
                    v.push(6);
                    v.push(i64::try_from(Version::of(&l.spec).0 >> 66).unwrap_or(0));
                    self.fields_into(core::slice::from_ref(&l.leader), reads, v);
                }
                Node::Kern { width, subtype, .. } => {
                    v.extend([7, i64::from(*width), i64::from(*subtype)]);
                }
                Node::MarginKern { width, left, .. } => {
                    v.extend([8, i64::from(*width), i64::from(*left)]);
                }
                Node::Penalty(p) => v.extend([9, i64::from(*p)]),
                Node::Math { width, subtype, .. } => {
                    v.extend([10, i64::from(*width), i64::from(*subtype)]);
                }
                Node::Disc(d) => {
                    let Disc { pre, post, replace } = &**d;
                    v.push(11);
                    for part in [pre, post, replace] {
                        v.push(i64::try_from(part.len()).unwrap_or(0));
                        self.fields_into(part, reads, v);
                    }
                }
                Node::Ins(i) => v.extend([12, i64::from(i.number), i64::from(i.height)]),
                Node::Mark(_) if reads.marks => {
                    v.push(13);
                    v.push(i64::try_from(Version::of(n).0 >> 66).unwrap_or(0));
                }
                Node::Mark(_) => v.push(13),
                Node::Adjust(_) => v.push(14),
                Node::Whatsit(_) if reads.letters => {
                    // (a language whatsit is read by hyphenation)
                    v.push(15);
                    v.push(i64::try_from(Version::of(n).0 >> 66).unwrap_or(0));
                }
                Node::Whatsit(_) => v.push(15),
                Node::Unset(u) => v.extend([
                    16,
                    i64::from(u.width),
                    i64::from(u.height),
                    i64::from(u.depth),
                ]),
            }
        }
    }
}
