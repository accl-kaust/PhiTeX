//! Display items (DESIGN.md §5.6): what the walk of a page or form asks
//! of the content-stream encoder, recorded while the walk runs and encoded
//! when the stream ends.
//!
//! The walk keeps everything TeX observes or that can print or fail:
//! `\write`s, late expansion, `\pdfsavepos`, the color and link stacks,
//! special parsing and its warnings, and a font's first use (which numbers
//! its object). The encoder keeps PDF's text state (the current font, text
//! matrix and string) and emits bytes. Encoding a page's items at its end
//! gives the same bytes as encoding each at once: pdfTeX writes no other
//! object while a content stream is open, and the walk never reads the
//! encoder's state.
//!
//! One mode is encoded at once: with `\pdfinterwordspaceon` the encoder
//! decides from its text state whether to load the `pdftexspace` font,
//! which TeX observes.

use alloc::vec::Vec;

use partex_engine::node::Dims;

use crate::arith::Scaled;
use crate::host::Host;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;

/// One request of the walk to the encoder.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Draw {
    /// Character `c` of font `f` (not a virtual one).
    Char { f: i32, c: u8 },
    /// A rule with its lower left corner at (`x`, `y`).
    Rule {
        x: Scaled,
        y: Scaled,
        w: Scaled,
        h: Scaled,
    },
    /// A literal's text in a resolved mode (`SET_ORIGIN`, `DIRECT_PAGE`
    /// or `DIRECT_ALWAYS`).
    Literal { text: Vec<u8>, mode: i32 },
    /// A form (`\pdfrefxform`).
    Form { objnum: i32 },
    /// An image (`\pdfrefximage`).
    Image { objnum: i32, dims: Dims },
    /// `\pdffakespace`.
    FakeSpace,
    /// End any text object (`pdf_end_text`).
    EndText,
}

partex_engine::persist_enum!(Draw {
    Char { f, c },
    Rule { x, y, w, h },
    Literal { text, mode },
    Form { objnum },
    Image { objnum, dims },
    FakeSpace,
    EndText
});

/// A display item with the walk's position when it was asked for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Drawn {
    pub h: Scaled,
    pub v: Scaled,
    pub item: Draw,
}

partex_engine::persist_struct!(Drawn { h, v, item });

impl Drawn {
    /// The polynomial `p` (`partex_engine::node::tok_poly_step`'s) with
    /// this item's words after it: its position, its kind, and its fields
    /// (a literal's length, then its bytes, four to a word), so that the
    /// words of a list of items say which items they are.
    fn fold(&self, p: u64) -> u64 {
        use partex_engine::node::tok_poly_step as step;
        let words = |p: u64, ws: &[i32]| ws.iter().fold(p, |p, &w| step(p, w));
        let p = words(p, &[self.h, self.v]);
        match &self.item {
            Draw::Char { f, c } => words(p, &[0, *f, i32::from(*c)]),
            Draw::Rule { x, y, w, h } => words(p, &[1, *x, *y, *w, *h]),
            Draw::Literal { text, mode } => {
                let n = i32::try_from(text.len()).unwrap_or(i32::MAX);
                let p = words(p, &[2, *mode, n]);
                text.chunks(4).fold(p, |p, c| {
                    let mut b = [0u8; 4];
                    b[..c.len()].copy_from_slice(c);
                    step(p, i32::from_le_bytes(b))
                })
            }
            Draw::Form { objnum } => words(p, &[3, *objnum]),
            Draw::Image { objnum, dims } => {
                words(p, &[4, *objnum, dims.width, dims.height, dims.depth])
            }
            Draw::FakeSpace => words(p, &[5]),
            Draw::EndText => words(p, &[6]),
        }
    }
}

/// The display items recorded and not encoded yet, with the polynomial
/// of their words ([`Drawn::fold`]) made as each is recorded: the ship
/// state's version hashes that, and the count, not every item again (a
/// recorded call's start versions the ship state, `val.rs`: each `\write`
/// of a page, hashing the items so far each time).
#[derive(Clone, Debug, Default)]
pub(crate) struct DrawnList {
    items: Vec<Drawn>,
    poly: u64,
}

impl DrawnList {
    pub(crate) fn push(&mut self, d: Drawn) {
        self.poly = d.fold(self.poly);
        self.items.push(d);
    }

    /// The items, the list left empty.
    pub(crate) fn take(&mut self) -> Vec<Drawn> {
        self.poly = 0;
        core::mem::take(&mut self.items)
    }
}

impl PartialEq for DrawnList {
    fn eq(&self, other: &Self) -> bool {
        self.items == other.items
    }
}

impl core::hash::Hash for DrawnList {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        (self.items.len(), self.poly).hash(h);
    }
}

impl partex_engine::persist::Persist for DrawnList {
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        self.items.save(s);
    }

    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        let items: Vec<Drawn> = partex_engine::persist::Persist::load(l)?;
        let poly = items.iter().fold(0, |p, d| d.fold(p));
        Some(DrawnList { items, poly })
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Ask the encoder for `item` at the walk's position: recorded while
    /// a content stream is open, else (and in `\pdfinterwordspaceon`
    /// mode) encoded at once.
    pub(crate) fn draw(&mut self, item: Draw) -> Result<(), Jump> {
        if let Draw::Char { f, .. } = item
            && !self.pdf_font(f).used
        {
            // a font's first use numbers its object: in the walk's order
            self.pdf_init_font(f)?;
        }
        let at_once = self.pdf.ship.faked_space || matches!(item, Draw::FakeSpace);
        if let Draw::Char { f, .. } = item
            && !at_once
        {
            // the page's font resources, in the order of first use (the
            // encoder, which adds them too, finds them there; at once, it
            // may add `pdftexspace` first)
            let k = self.pdf_ff(f);
            let list = core::mem::take(&mut self.pdf.ship.font_list);
            let known = list.iter().any(|&g| self.pdf_ff(g) == k);
            self.pdf.ship.font_list = list;
            if !known {
                self.pdf.ship.font_list.push(f);
            }
        }
        let drawn = Drawn {
            h: self.pdf.ship.cur_h,
            v: self.pdf.ship.cur_v,
            item,
        };
        if self.pdf.ship.recording && !at_once {
            self.pdf.ship.drawn.push(drawn);
            return Ok(());
        }
        self.flush_drawn()?;
        self.encode(&drawn)
    }

    /// Encode the recorded items.
    pub(crate) fn flush_drawn(&mut self) -> Result<(), Jump> {
        let items = self.pdf.ship.drawn.take();
        for d in &items {
            self.encode(d)?;
        }
        Ok(())
    }

    /// Encode one item at its position (the walk's position is kept).
    fn encode(&mut self, d: &Drawn) -> Result<(), Jump> {
        let (h, v) = (self.pdf.ship.cur_h, self.pdf.ship.cur_v);
        self.pdf.ship.cur_h = d.h;
        self.pdf.ship.cur_v = d.v;
        let r = match &d.item {
            Draw::Char { f, c } => self.emit_char(*f, *c),
            Draw::Rule { x, y, w, h } => self.pdf_set_rule(*x, *y, *w, *h),
            Draw::Literal { text, mode } => self.emit_literal(text, *mode),
            Draw::Form { objnum } => self.emit_form(*objnum),
            Draw::Image { objnum, dims } => self.emit_image(*objnum, *dims),
            Draw::FakeSpace => self.pdf_insert_fake_space(),
            Draw::EndText => {
                self.pdf_end_text();
                Ok(())
            }
        };
        self.pdf.ship.cur_h = h;
        self.pdf.ship.cur_v = v;
        r
    }
}
