//! The page IR: an owned, immutable description of one shipped-out page.
//!
//! `ship_out` walks the box once in the engine (executing `\write`,
//! `\openout` and `\closeout` in tex.web's order, resolving `\special`
//! texts, glue setting, running rule dimensions and `MLTeX` substitutions)
//! and produces a [`Page`]. Output backends (`dviout`) consume pages
//! without touching engine state, so they can run anywhere.
//!
//! The tree is flat: a box is an [`Item::Box`] header followed by the
//! `len` items of its contents, so a page is one vector of `Copy` items
//! (cheap to build, drop, hash, compare and send to another thread).
//!
//! Positions are not resolved here: a backend walks the tree as tex.web's
//! `hlist_out`/`vlist_out` (§619, §629) do, which keeps DVI movement
//! optimization (and leader alignment) byte-identical.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use crate::Scaled;

/// One page, as `ship_out` sees it (§640).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Page {
    /// `\count0` … `\count9`.
    pub counts: [i32; 10],
    pub h_offset: Scaled,
    pub v_offset: Scaled,
    /// The shipped box (`items[0]`, whose `shift` is ignored, §640) and
    /// its contents.
    pub items: Vec<Item>,
    /// The bytes of the page's `\special`s.
    pub specials: Vec<u8>,
    /// Definitions of the fonts this page uses (a backend emits each font's
    /// definition once, at first use).
    pub fonts: Vec<FontDef>,
    /// The walk stopped early (at an [`Item::Cut`]).
    pub truncated: bool,
}

crate::persist_struct!(Page {
    counts,
    h_offset,
    v_offset,
    items,
    specials,
    fonts,
    truncated
});

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LeaderKind {
    Aligned,
    Centered,
    Expanded,
}

crate::persist_enum!(LeaderKind {
    Aligned,
    Centered,
    Expanded
});

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    /// Typeset character `ch` of internal font `font`, advancing by `width`;
    /// `raise` moves it up relative to the baseline (`MLTeX` accents).
    Char {
        font: i32,
        ch: i32,
        width: Scaled,
        raise: Scaled,
    },
    /// A character the font lacks: tex.web still synchronizes the position
    /// and selects the font (§620), but sets nothing.
    Missing { font: i32 },
    /// Move right (hlist) or down (vlist): kerns, set glue, math nodes and
    /// empty boxes.
    Move(Scaled),
    /// A rule with running dimensions resolved. In an hlist it is set with
    /// its bottom at `depth` below the baseline (§624); in a vlist it is
    /// put below the current position (§633).
    Rule {
        height: Scaled,
        depth: Scaled,
        width: Scaled,
    },
    /// An `hlist_node` or `vlist_node` with a non-empty list, followed by
    /// the `len` items of its contents. (Empty boxes are plain
    /// [`Item::Move`]s: tex.web only moves past them.)
    Box {
        vertical: bool,
        width: Scaled,
        height: Scaled,
        depth: Scaled,
        shift: Scaled,
        len: u32,
        /// e-TeX: a display line (`dlist`), never reflected.
        display: bool,
    },
    /// e-TeX's `TeXXeT`: an edge of a reflected segment of an hlist: move
    /// right by `width`, then output to the right of here is in direction
    /// `rtl`, and leaders align with here plus `dist`.
    Edge {
        width: Scaled,
        dist: Scaled,
        rtl: bool,
    },
    /// Box leaders filling `size` (the glue's set size), followed by the
    /// leader box.
    Leaders { kind: LeaderKind, size: Scaled },
    /// A `\special`: `len` bytes at `start` of [`Page::specials`].
    Special { start: u32, len: u32 },
    /// The walk stopped here (a `\write` ended the job): tex.web leaves
    /// this box and those around it open, for `finish_dvi_file` to close
    /// with bare `pop`s and an `eop` (§642).
    Cut,
}

crate::persist_enum!(Item { Char { font, ch, width, raise }, Missing { font }, Move(a0), Rule { height, depth, width }, Box { vertical, width, height, depth, shift, len, display }, Edge { width, dist, rtl }, Leaders { kind, size }, Special { start, len }, Cut });

/// What a DVI `fnt_def` says about a font (§602).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FontDef {
    pub font: i32,
    pub check: [u8; 4],
    pub size: Scaled,
    pub design_size: Scaled,
    pub area: Vec<u8>,
    pub name: Vec<u8>,
}

crate::persist_struct!(FontDef {
    font,
    check,
    size,
    design_size,
    area,
    name
});

impl Page {
    /// Empty the page, keeping its buffers.
    pub fn clear(&mut self) {
        self.items.clear();
        self.specials.clear();
        self.fonts.clear();
        self.truncated = false;
    }

    /// The bytes of an [`Item::Special`].
    #[must_use]
    pub fn special(&self, start: u32, len: u32) -> &[u8] {
        &self.specials[start as usize..(start + len) as usize]
    }

    /// A stable text form of the page, for tests and debugging.
    #[must_use]
    pub fn dump(&self) -> String {
        let mut s = String::new();
        let _ = write!(s, "page {:?}", self.counts);
        let _ = writeln!(
            s,
            " offset=({},{}){}",
            self.h_offset,
            self.v_offset,
            if self.truncated { " truncated" } else { "" }
        );
        for f in &self.fonts {
            let _ = writeln!(
                s,
                "font {} {}{} at {} design {} check {:02x?}",
                f.font,
                String::from_utf8_lossy(&f.area),
                String::from_utf8_lossy(&f.name),
                f.size,
                f.design_size,
                f.check
            );
        }
        let mut ends: Vec<usize> = Vec::new(); // where open boxes end
        for (i, item) in self.items.iter().enumerate() {
            while ends.last() == Some(&i) {
                ends.pop();
            }
            let ind = ends.len() * 2;
            match *item {
                Item::Char {
                    font,
                    ch,
                    width,
                    raise,
                } => {
                    let _ = write!(s, "{:ind$}char f{font} {ch} wd={width}", "");
                    if raise != 0 {
                        let _ = write!(s, " raise={raise}");
                    }
                    s.push('\n');
                }
                Item::Missing { font } => {
                    let _ = writeln!(s, "{:ind$}missing f{font}", "");
                }
                Item::Move(d) => {
                    let _ = writeln!(s, "{:ind$}move {d}", "");
                }
                Item::Rule {
                    height,
                    depth,
                    width,
                } => {
                    let _ = writeln!(s, "{:ind$}rule ht={height} dp={depth} wd={width}", "");
                }
                Item::Box {
                    vertical,
                    width,
                    height,
                    depth,
                    shift,
                    len,
                    display,
                } => {
                    let _ = writeln!(
                        s,
                        "{:ind$}{} wd={width} ht={height} dp={depth} shift={shift}{}",
                        "",
                        if vertical { "vbox" } else { "hbox" },
                        if display { " display" } else { "" },
                    );
                    ends.push(i + 1 + len as usize);
                }
                Item::Leaders { kind, size } => {
                    let _ = writeln!(s, "{:ind$}leaders {kind:?} size={size}", "");
                }
                Item::Special { start, len } => {
                    let text = String::from_utf8_lossy(self.special(start, len));
                    let _ = writeln!(s, "{:ind$}special {text:?}", "");
                }
                Item::Edge { width, dist, rtl } => {
                    let dir = if rtl { "rtl" } else { "ltr" };
                    let _ = writeln!(s, "{:ind$}edge {width} dist={dist} {dir}", "");
                }
                Item::Cut => {
                    let _ = writeln!(s, "{:ind$}cut", "");
                }
            }
        }
        s
    }
}
