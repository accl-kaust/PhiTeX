//! A page's glyphs as the writer shows them, for glyph origins
//! (docs/note-origins.md §2.1): set with [`PdfState::set_tap`], the
//! writer tells a [`GlyphTap`] every code its content stream shows, in
//! the stream's order. Unset (the default), nothing is called and the
//! bytes are the same either way.

use std::sync::{Arc, Mutex, PoisonError};

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
    /// A form or an included page drawn (`Do`), object `obj`: its codes
    /// are its own stream's, told by [`GlyphTap::xobject_codes`] before
    /// the page's writing ends (untold: not known).
    fn xobject(&mut self, obj: usize);
    /// Object `obj`, drawn on the page, shows `codes` codes where it is
    /// drawn (the forms and pages it draws counted).
    fn xobject_codes(&mut self, obj: usize, codes: usize) {
        let _ = (obj, codes);
    }
    /// A native word or glyph of XeTeX's XDV file (`font` `size` big):
    /// its text in UTF-16 units (empty: a glyph, `\XeTeXglyph`), and
    /// each glyph's position (sp from the page's top left) and advance;
    /// the PDF made from the file (xdvipdfmx) shows one code a glyph.
    /// Untold, its codes are none.
    fn native(
        &mut self,
        font: FontId,
        text: &[u16],
        glyphs: &[(Scaled, Scaled, Scaled)],
        size: Scaled,
    ) {
        let _ = (font, text, size);
        self.none(glyphs.len());
    }
    /// The codes from here on are at reversal level `level` (TeX--XeT's
    /// segments, as the bidi algorithm's embedding levels: 0 outside
    /// them, odd in right-to-left text, the next even in left-to-right
    /// text inside it): the writer shows a reversed segment's codes in
    /// the page's order, right to left in the reading. A level that
    /// goes back to 0 ends a context (a line, a box of lines) whose
    /// codes are read again in their order on their own.
    fn level(&mut self, level: u8) {
        let _ = level;
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
    /// The reversal level last told ([`GlyphTap::level`]).
    pub(crate) level: u8,
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

    /// The reversal level is `l` from here on, told the sink when it
    /// changes ([`GlyphTap::level`]); nothing without a sink.
    #[inline]
    pub(crate) fn set_level(&mut self, l: u8) {
        if let Some(s) = self.sink.as_mut()
            && self.level != l
        {
            self.level = l;
            s.level(l);
        }
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
        self.tap.level = 0;
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

/// The codes a literal's bytes show (`\\pdfliteral`, a special's
/// content): its text's, counted as a reader of the stream counts them.
#[must_use]
pub fn literal_codes(bytes: &[u8]) -> usize {
    struct Count(usize);
    impl phitex_codecs::pdftext::Text for Count {
        fn font(&mut self, _: &[u8]) -> phitex_codecs::pdftext::Font {
            phitex_codecs::pdftext::Font::default()
        }
        fn code(&mut self, _: &[u8], _: u32, _: f64, _: f64) {
            self.0 += 1;
        }
        fn draw(&mut self, _: &[u8], _: &phitex_codecs::pdftext::State) {}
    }
    // (a literal showing text: rare; most set colours)
    if !bytes.iter().any(|&b| b == b'T' || b == b'\'' || b == b'"') {
        return 0;
    }
    let mut c = Count(0);
    phitex_codecs::pdftext::walk(bytes, &mut c);
    c.0
}

/// A form's codes as its stream is written: its own, and the objects
/// it draws whose codes are not told yet (forms written after it).
#[derive(Default)]
struct Counted {
    n: usize,
    drawn: Vec<usize>,
}

/// The tap a form's writing tells ([`PdfState::ship_form_box`]).
struct Count(Arc<Mutex<Counted>>);

impl Count {
    fn with(&self, f: impl FnOnce(&mut Counted)) {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner));
    }
}

impl GlyphTap for Count {
    fn code(
        &mut self,
        _: (FontId, u16),
        _: &[u16],
        _: bool,
        _: FontId,
        _: u8,
        _: Scaled,
        _: Scaled,
        _: Scaled,
        _: Scaled,
    ) {
        self.with(|c| c.n += 1);
    }
    fn none(&mut self, n: usize) {
        self.with(|c| c.n += n);
    }
    fn literal(&mut self, bytes: &[u8]) {
        let k = literal_codes(bytes);
        self.with(|c| c.n += k);
    }
    fn xobject(&mut self, obj: usize) {
        self.with(|c| c.drawn.push(obj));
    }
    fn xobject_codes(&mut self, obj: usize, codes: usize) {
        self.with(|c| {
            if let Some(i) = c.drawn.iter().rposition(|&o| o == obj) {
                c.drawn.remove(i);
                c.n += codes;
            }
        });
    }
}

impl PdfState {
    /// Ships form `k`'s box `b` (`pdf_ship_out(b, false)`) and keeps the
    /// codes it shows where it is drawn ([`crate::objects::XForm`]'s
    /// `codes`): told the tap of each page that draws it, the form's
    /// codes are its own whichever page wrote it. Its writing tells the
    /// page's tap nothing.
    pub(crate) fn ship_form_box(
        &mut self,
        k: usize,
        b: &phitex_typeset::node::BoxNode,
        src: &dyn crate::FontSource,
        wh: &mut crate::WhatsitSource<'_>,
    ) -> Result<(), crate::PdfError> {
        let c = Arc::new(Mutex::new(Counted::default()));
        let level = self.tap.level;
        let saved = self.set_tap(Some(Box::new(Count(c.clone()))));
        let r = self.ship(b, false, src, wh);
        self.set_tap(saved);
        self.tap.level = level;
        let c = std::mem::take(&mut *c.lock().unwrap_or_else(PoisonError::into_inner));
        let n = c.n
            + c.drawn
                .iter()
                .map(|o| self.aux.xforms.get(o).and_then(|x| x.codes).unwrap_or(0))
                .sum::<usize>();
        if let Some(x) = self.aux.xforms.get_mut(&k) {
            x.codes = Some(n);
        }
        r
    }

    /// The codes each form or image record `rec` leaves to its placing
    /// shows (`tex` naming its objects here), in the page's order: a
    /// form's once written here, an image's once read; `None` where not
    /// known.
    #[must_use]
    pub fn hole_codes(
        &self,
        rec: &crate::PageRecord,
        tex: &dyn Fn(usize) -> usize,
    ) -> Vec<Option<usize>> {
        rec.xholes
            .iter()
            .map(|x| {
                let n = tex(x.n);
                if x.form {
                    self.aux.xforms.get(&n).and_then(|f| f.codes)
                } else {
                    self.ximage_codes(n)
                }
            })
            .collect()
    }

    /// Tells the tap the codes of the forms the page drew.
    pub(crate) fn tap_forms(&mut self) {
        if !self.tap.on() {
            return;
        }
        for k in self.xform_list.clone() {
            let n = self.aux.xforms.get(&k).and_then(|x| x.codes);
            if let (Some(n), Some(t)) = (n, self.tap.sink.as_mut()) {
                t.xobject_codes(k, n);
            }
        }
    }
}
