//! A page's glyphs as the writer shows them, for glyph origins
//! (docs/note-origins.md §2.1): set with [`PdfState::set_tap`], the
//! writer tells a [`GlyphTap`] every code its content stream shows, in
//! the stream's order. Unset (the default), nothing is called and the
//! bytes are the same either way.

use phitex_typeset::arith::Scaled;
use phitex_typeset::fonts::FontId;

use crate::PdfState;

/// What the page's writer tells the tap.
pub trait GlyphTap: Send + Sync {
    /// A code shown for the character node being written (`node`: its
    /// font and character, `lig` the characters a ligature replaces):
    /// font `f`'s code `c`, at (`h`, `v`) (sp from the page's top left),
    /// `w` wide, the font `size` big; `first`: the node's first code (a
    /// virtual font's character can show several).
    #[allow(clippy::too_many_arguments)]
    fn code(
        &mut self,
        node: (FontId, u16),
        lig: &[u16],
        first: bool,
        f: FontId,
        c: u8,
        h: Scaled,
        v: Scaled,
        w: Scaled,
        size: Scaled,
    );
    /// `n` codes no character node made (pdfTeX's interword and fake
    /// spaces; a code written outside a character's node).
    fn none(&mut self, n: usize);
    /// A literal's bytes (`\pdfliteral`, `\special{pdf:...}`, a virtual
    /// font's special): the codes it shows are the tap's to count.
    fn literal(&mut self, bytes: &[u8]);
    /// A form or an included page drawn (`Do`): its codes are its own
    /// stream's.
    fn xobject(&mut self);
    /// A native word or glyph of XeTeX's XDV file (`font` `size` big):
    /// its text in UTF-16 units (empty: a glyph, `\XeTeXglyph`), and
    /// each glyph's position (sp from the page's top left) and advance;
    /// the PDF made from the file (xdvipdfmx) shows one code a glyph.
    /// Untold, its codes are none.
    fn native(&mut self, font: FontId, text: &[u16], glyphs: &[(Scaled, Scaled, Scaled)], size: Scaled) {
        let _ = (font, text, size);
        self.none(glyphs.len());
    }
}

/// The writer's tap and the character node being written: outside the
/// state's digest, and not cloned (a clone writes nothing to it).
#[derive(Default)]
pub(crate) struct Tap {
    pub sink: Option<Box<dyn GlyphTap>>,
    node: Option<(FontId, u16)>,
    lig: Vec<u16>,
    /// No code told for `node` yet.
    first: bool,
}

impl Clone for Tap {
    fn clone(&self) -> Tap {
        Tap::default()
    }
}

impl std::fmt::Debug for Tap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("tap")
    }
}

impl Tap {
    #[inline]
    pub(crate) fn on(&self) -> bool {
        self.sink.is_some()
    }

    /// The character node whose codes follow (`None`: none).
    pub(crate) fn node(&mut self, n: Option<(FontId, u16)>, lig: &[u16]) {
        self.node = n;
        self.first = true;
        self.lig.clear();
        self.lig.extend_from_slice(lig);
    }
}

impl PdfState {
    /// Sets the tap the page writer tells its glyphs (`None`: none), and
    /// gives back the one set before.
    pub fn set_tap(&mut self, t: Option<Box<dyn GlyphTap>>) -> Option<Box<dyn GlyphTap>> {
        self.tap.node(None, &[]);
        std::mem::replace(&mut self.tap.sink, t)
    }

    /// Font `f`'s code `c` written at the current position.
    pub(crate) fn tap_code(&mut self, c: u8) {
        let f = self.pdf_f;
        let (w, size) = self
            .fonts
            .get(&f)
            .map_or((0, 0), |st| (st.width(u16::from(c)), st.size));
        let (h, v) = (self.cur_h, self.cur_v);
        let t = &mut self.tap;
        if let Some(s) = t.sink.as_mut() {
            match t.node {
                Some(n) => {
                    s.code(n, &t.lig, t.first, f, c, h, v, w, size);
                    t.first = false;
                }
                None => s.none(1),
            }
        }
    }
}
