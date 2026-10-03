//! pdfTeX's font expansion and character protrusion, and the character
//! codes that drive them (`\lpcode`, `\efcode`, `\tagcode`, …).

use partex_engine::font::Tag;
use partex_engine::web::{NO_LIG_CODE, TAG_CODE};

use crate::fonts::{Code, fx};
use crate::host::Host;
use crate::print::NEW_STRING;
use crate::tex::{Jump, Tex};
use crate::track::{Cell, Tracker, font as field};
use crate::web::{FONT_ID_BASE, NULL_CS, NULL_FONT, SET_FONT};

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §426 with pdfTeX's font integers: the value of `\hyphenchar`,
    /// `\lpcode` … (`m` is the primitive's code).
    pub(crate) fn fetch_font_int(&mut self, m: i32) -> Result<i32, Jump> {
        self.scan_font_ident()?;
        let f = self.cur_val;
        self.tracker.read(Cell::Font(f));
        Ok(match m {
            0 => {
                self.font_read(f, field::HYPHEN_CHAR);
                self.fonts.hyphen_char[fx(f)]
            }
            1 => {
                self.font_read(f, field::SKEW_CHAR);
                self.fonts.skew_char[fx(f)]
            }
            NO_LIG_CODE => self.test_no_ligatures(f),
            _ => {
                self.scan_char_num()?;
                let c = self.cur_val;
                match Code::of_chr(m) {
                    Some(code) => self.font_code(f, code, byte(c)),
                    None => self.tag_code(f, c),
                }
            }
        })
    }

    /// Code `code` of character `c` of font `f` (pdfTeX's `get_lp_code`
    /// …): an expanded font's are its base font's, so a read of its
    /// expansion and of the base font's table.
    pub(crate) fn font_code(&self, f: i32, code: Code, c: u8) -> i32 {
        self.font_read(f, field::EXPAND);
        self.font_read(self.fonts.base(f), field::CODES + code as u32);
        self.fonts.code(f, code, c)
    }

    /// §1253 with pdfTeX's font integers: assign `\hyphenchar`, `\lpcode`
    /// … (global, like `\hyphenchar`).
    pub(crate) fn assign_font_int(&mut self, n: i32) -> Result<(), Jump> {
        self.scan_font_ident()?;
        let f = self.cur_val;
        // (one field of the font's state: the rest stays)
        self.tracker.read(Cell::Font(f));
        self.tracker.write(Cell::Font(f));
        if n == NO_LIG_CODE {
            self.set_no_ligatures(f);
        } else if n < 2 {
            self.scan_optional_equals()?;
            self.scan_int()?;
            if n == 0 {
                self.fonts.hyphen_char[fx(f)] = self.cur_val;
                self.font_wrote(f, field::HYPHEN_CHAR);
            } else {
                self.fonts.skew_char[fx(f)] = self.cur_val;
                self.font_wrote(f, field::SKEW_CHAR);
            }
        } else {
            self.scan_char_num()?;
            let c = self.cur_val;
            self.scan_optional_equals()?;
            self.scan_int()?;
            match Code::of_chr(n) {
                Some(code) => {
                    self.fonts.set_code(f, code, byte(c), self.cur_val);
                    self.font_wrote(f, field::CODES + code as u32);
                }
                None if n == TAG_CODE => self.set_tag_code(f, c, self.cur_val),
                None => {}
            }
        }
        Ok(())
    }

    /// pdfTeX's `expand_font_name`: `f`'s name with `+e` or `-e`, a new
    /// string.
    fn expand_font_name(&mut self, f: i32, e: i32) -> Result<i32, Jump> {
        let old_setting = self.selector();
        self.set_selector(NEW_STRING);
        self.font_read(f, field::METRICS);
        self.print(self.fonts.name[fx(f)]);
        if e > 0 {
            self.print_char(b'+');
        }
        self.print_int(e);
        self.set_selector(old_setting);
        let s = self.make_string()?;
        Ok(i32::try_from(s).unwrap_or(0))
    }

    /// Font `f`'s state is about to change in part (which depends on the
    /// rest).
    fn change_font(&self, f: i32) {
        self.tracker.read(Cell::Font(f));
        self.tracker.write(Cell::Font(f));
    }

    /// Font `k` is about to be made from font `f`.
    fn new_font_cells(&self, f: i32, k: i32) {
        self.tracker.read(Cell::FontTable);
        self.tracker.read(Cell::Font(f));
        self.tracker.write(Cell::FontTable);
        self.tracker.write(Cell::Font(k));
    }

    /// pdfTeX's `auto_expand_font`: font `f` expanded by `e` thousandths
    /// as a new font, from `f`'s metrics.
    pub(crate) fn auto_expand_font(&mut self, f: i32, e: i32) -> Result<i32, Jump> {
        self.font_table_read();
        if self.font_ptr >= self.params.font_max {
            return self.overflow(
                b"maximum internal font number (font_max)",
                self.params.font_max,
            );
        }
        let name = self.expand_font_name(f, e)?;
        let nb = self.str_bytes(crate::input::ux(name)).to_vec();
        let (k, ident, idv) = self.new_font_slot(
            &crate::fonts::FontIdent::Expanded { base: f, ratio: e },
            &nb,
        );
        self.new_font_cells(f, k);
        // (the new font is made from `f`'s metrics and settable fields)
        for x in [
            field::METRICS,
            field::HYPHEN_CHAR,
            field::SKEW_CHAR,
            field::GLUE,
        ] {
            self.font_read(f, x);
        }
        let (i, fonts) = (fx(f), &mut self.fonts);
        let metrics = fonts.metrics[i].expanded(e);
        let (tfm, area) = (fonts.tfm[i].clone(), fonts.area[i]);
        fonts.place(k, metrics, tfm, name, area, ident, idv);
        let j = fx(k);
        fonts.retagged[j] = fonts.retagged[i];
        fonts.remade[j] = fonts.remade[i];
        fonts.hyphen_char[j] = fonts.hyphen_char[i];
        fonts.skew_char[j] = fonts.skew_char[i];
        fonts.glue[j] = fonts.glue[i];
        self.font_ptr += 1;
        self.font_made(k);
        self.font_named(name);
        self.font_loaded(k);
        let id = self.font_id_text(f);
        self.set_font_id_text(k, id);
        Ok(k)
    }

    /// pdfTeX's `copy_expand_params`: expanded font `k` is `f` expanded by
    /// `e` (its character codes are `f`'s, [`FontArrays::code`]).
    pub(crate) fn copy_expand_params(&mut self, k: i32, f: i32, e: i32) {
        self.tracker.read(Cell::Font(f));
        self.change_font(k);
        self.font_read(f, field::EXPAND);
        let x = self.fonts.expand[fx(f)];
        let y = &mut self.fonts.expand[fx(k)];
        y.ratio = e;
        y.step = x.step;
        y.auto = x.auto;
        y.blink = f;
        self.font_wrote(k, field::EXPAND);
    }

    /// pdfTeX's `load_expand_font`.
    fn load_expand_font(&mut self, f: i32, e: i32) -> Result<i32, Jump> {
        let s = self.expand_font_name(f, e)?;
        let size = self.fonts.metrics[fx(f)].size;
        let mut k = self.tfm_lookup(s, size);
        if k == NULL_FONT {
            self.font_read(f, field::EXPAND);
            k = if self.fonts.expand[fx(f)].auto {
                self.auto_expand_font(f, e)?
            } else {
                let empty = self.pool_str(b"");
                self.read_font_info(NULL_CS, s, empty, size)?
            };
        }
        if k != NULL_FONT {
            self.copy_expand_params(k, f, e);
        }
        Ok(k)
    }

    /// pdfTeX's `get_expand_font`: `f` expanded by `e`, loaded if new.
    ///
    /// (pdfTeX searches `f`'s chain of expanded fonts, `pdf_font_elink`,
    /// for the ratio; the fonts on it have distinct ratios, so the chain
    /// is kept as a map by base font and ratio.)
    pub(crate) fn get_expand_font(&mut self, f: i32, e: i32) -> Result<i32, Jump> {
        self.tracker.read(Cell::Font(f));
        // (the chain of `f`'s expanded fonts is a part of its expansion)
        self.font_read(f, field::EXPAND);
        self.font_touch(crate::fonts::FontTouch::Expand(f, e, false));
        if let Some(&k) = self.fonts.expanded.get(&(f, e)) {
            self.tracker.read(Cell::Font(k));
            return Ok(k);
        }
        let k = self.load_expand_font(f, e)?;
        self.change_font(f);
        self.change_font(k);
        if k == NULL_FONT {
            // (the null font put at the chain's head ends it there: the
            // fonts on it are no longer found)
            let gone: alloc::vec::Vec<i32> = self
                .fonts
                .expanded
                .keys()
                .filter(|&&(b, _)| b == f)
                .map(|&(_, r)| r)
                .collect();
            for r in gone {
                self.fonts.expanded.remove(&(f, r));
                self.font_touch(crate::fonts::FontTouch::Expand(f, r, true));
            }
        } else {
            self.fonts.expanded.insert((f, e), k);
            self.font_touch(crate::fonts::FontTouch::Expand(f, e, true));
        }
        self.font_wrote(f, field::EXPAND);
        Ok(k)
    }

    /// pdfTeX's `set_expand_params`.
    pub(crate) fn set_expand_params(
        &mut self,
        f: i32,
        auto: bool,
        stretch_limit: i32,
        shrink_limit: i32,
        step: i32,
        ratio: i32,
    ) -> Result<(), Jump> {
        self.change_font(f);
        self.fonts.expand[fx(f)].step = step;
        self.fonts.expand[fx(f)].auto = auto;
        self.font_wrote(f, field::EXPAND);
        if stretch_limit > 0 {
            let k = self.get_expand_font(f, stretch_limit)?;
            self.fonts.expand[fx(f)].stretch = k;
            self.font_wrote(f, field::EXPAND);
        }
        if shrink_limit > 0 {
            let k = self.get_expand_font(f, -shrink_limit)?;
            self.fonts.expand[fx(f)].shrink = k;
            self.font_wrote(f, field::EXPAND);
        }
        if ratio != 0 {
            self.fonts.expand[fx(f)].ratio = ratio;
            self.font_wrote(f, field::EXPAND);
        }
        Ok(())
    }

    /// The stretch and shrink limits of expandable font `f`.
    pub(crate) fn expand_limits(&self, f: i32) -> (i32, i32) {
        self.font_read(f, field::EXPAND);
        let x = self.fonts.expand[fx(f)];
        for g in [f, x.stretch, x.shrink] {
            self.tracker.read(Cell::Font(g));
            self.font_read(g, field::EXPAND);
        }
        let ratio = |k: i32| self.fonts.expand[fx(k)].ratio;
        (ratio(x.stretch), -ratio(x.shrink))
    }

    /// pdfTeX's `read_expand_font`: `\pdffontexpand`.
    pub(crate) fn read_expand_font(&mut self) -> Result<(), Jump> {
        self.scan_font_ident()?;
        let f = self.cur_val;
        if f == NULL_FONT {
            return self.pdf_error(b"font expansion", b"invalid font identifier");
        }
        self.font_read(f, field::EXPAND);
        if self.fonts.expand[fx(f)].blink != NULL_FONT {
            return self.pdf_error(
                b"font expansion",
                b"\\pdffontexpand cannot be used this way (the base font has been expanded)",
            );
        }
        self.scan_optional_equals()?;
        self.scan_int()?;
        let mut stretch_limit = self.cur_val.clamp(0, 1000);
        self.scan_int()?;
        let mut shrink_limit = self.cur_val.clamp(0, 500);
        self.scan_int()?;
        let step = self.cur_val.clamp(0, 100);
        if step == 0 {
            return self.pdf_error(b"font expansion", b"invalid step");
        }
        stretch_limit -= stretch_limit % step;
        shrink_limit -= shrink_limit % step;
        if stretch_limit == 0 && shrink_limit == 0 {
            return self.pdf_error(b"font expansion", b"invalid limit(s)");
        }
        let auto = self.scan_keyword(b"autoexpand")?;
        if auto {
            self.scan_optional_space()?;
        }
        // check if the font can be expanded
        let x = self.fonts.expand[fx(f)];
        if x.ratio != 0 {
            return self.pdf_error(
                b"font expansion",
                b"this font has been expanded by another font so it cannot be used now",
            );
        }
        if x.step != 0 {
            // this font has been expanded: the parameters must be the same
            self.font_read(x.stretch, field::EXPAND);
            self.font_read(x.shrink, field::EXPAND);
            let ratio = |k: i32| self.fonts.expand[fx(k)].ratio;
            if x.step != step {
                return self.pdf_error(
                    b"font expansion",
                    b"font has been expanded with different expansion step",
                );
            }
            if (x.stretch == NULL_FONT && stretch_limit != 0)
                || (x.stretch != NULL_FONT && ratio(x.stretch) != stretch_limit)
            {
                return self.pdf_error(
                    b"font expansion",
                    b"font has been expanded with different stretch limit",
                );
            }
            if (x.shrink == NULL_FONT && shrink_limit != 0)
                || (x.shrink != NULL_FONT && -ratio(x.shrink) != shrink_limit)
            {
                return self.pdf_error(
                    b"font expansion",
                    b"font has been expanded with different shrink limit",
                );
            }
            if x.auto != auto {
                return self.pdf_error(
                    b"font expansion",
                    b"font has been expanded with different auto expansion value",
                );
            }
        } else {
            let t = self.pdf_font_ref(f).font_type.clone();
            let virtual_font = matches!(t, crate::pdf::vf::FontType::Virtual(_));
            if t != crate::pdf::vf::FontType::New && !virtual_font {
                self.pdf_warning(
                    b"font expansion",
                    b"font should be expanded before its first use",
                    true,
                    true,
                );
            }
            self.set_expand_params(f, auto, stretch_limit, shrink_limit, step, 0)?;
            if virtual_font {
                self.vf_expand_local_fonts(f)?;
            }
        }
        Ok(())
    }

    /// pdfTeX's `new_letterspaced_font`: `\\letterspacefont`.
    pub(crate) fn new_letterspaced_font(&mut self, a: i32) -> Result<(), Jump> {
        let (u, t) = self.font_identifier()?;
        self.define(a, u, SET_FONT, NULL_FONT)?;
        self.scan_optional_equals()?;
        self.scan_font_ident()?;
        let k = self.cur_val;
        self.scan_int()?;
        let f = self.letter_space_font(u, k, self.cur_val.clamp(-1000, 1000))?;
        self.set_font_identifier(u, f, t);
        Ok(())
    }

    /// pdfTeX's `make_font_copy`: `\\pdfcopyfont`.
    pub(crate) fn make_font_copy(&mut self, a: i32) -> Result<(), Jump> {
        let (u, t) = self.font_identifier()?;
        self.define(a, u, SET_FONT, NULL_FONT)?;
        self.scan_optional_equals()?;
        self.scan_font_ident()?;
        let k = self.cur_val;
        let f = self.copy_font_info(k)?;
        self.set_font_identifier(u, f, t);
        Ok(())
    }

    /// `equiv(u):=f; eqtb[font_id_base+f]:=eqtb[u]; font_id_text(f):=t`.
    fn set_font_identifier(&mut self, u: i32, f: i32, t: i32) {
        self.set_equiv(u, f);
        let w = self.eqtb(u);
        self.set_eqtb(FONT_ID_BASE + f, w);
        self.set_font_id_text(f, t);
    }

    /// pdfTeX's `copy_font_info`: a copy of font `f` (for expanding it
    /// differently), which font lookups pass over.
    fn copy_font_info(&mut self, f: i32) -> Result<i32, Jump> {
        self.font_read(f, field::EXPAND);
        let x = self.fonts.expand[fx(f)];
        if x.ratio != 0 || x.step != 0 {
            return self.pdf_error(b"\\pdfcopyfont", b"cannot copy an expanded font");
        }
        if self.is_letterspaced_font(f) {
            return self.pdf_error(b"\\pdfcopyfont", b"cannot copy a letterspaced font");
        }
        self.font_table_read();
        if self.font_ptr >= self.params.font_max {
            return self.overflow(
                b"maximum internal font number (font_max)",
                self.params.font_max,
            );
        }
        for x in [
            field::METRICS,
            field::HYPHEN_CHAR,
            field::SKEW_CHAR,
            field::GLUE,
        ] {
            self.font_read(f, x);
        }
        let nb = self
            .str_bytes(crate::input::ux(self.fonts.name[fx(f)]))
            .to_vec();
        let (k, ident, idv) = self.new_font_slot(&crate::fonts::FontIdent::Copied { from: f }, &nb);
        self.new_font_cells(f, k);
        let area = self.pool_str(b"///...");
        let (i, fonts) = (fx(f), &mut self.fonts);
        let (metrics, tfm, name) = (
            (*fonts.metrics[i]).clone(),
            fonts.tfm[i].clone(),
            fonts.name[i],
        );
        fonts.place(k, metrics, tfm, name, area, ident, idv);
        let j = fx(k);
        fonts.retagged[j] = fonts.retagged[i];
        fonts.remade[j] = fonts.remade[i];
        fonts.hyphen_char[j] = fonts.hyphen_char[i];
        fonts.skew_char[j] = fonts.skew_char[i];
        fonts.glue[j] = fonts.glue[i];
        self.font_ptr += 1;
        self.font_made(k);
        self.font_named(name);
        self.font_loaded(k);
        Ok(k)
    }

    /// pdfTeX's `is_letterspaced_font`: a virtual font named like
    /// `name+100ls`.
    fn is_letterspaced_font(&mut self, f: i32) -> bool {
        if !matches!(
            self.pdf_font_ref(f).font_type,
            crate::pdf::vf::FontType::Virtual(_)
        ) {
            return false;
        }
        self.font_read(f, field::METRICS);
        let name = self.font_name_bytes(f);
        let Some(rest) = name.strip_suffix(b"ls") else {
            return false;
        };
        let digits = rest.iter().rev().take_while(|c| c.is_ascii_digit()).count();
        matches!(
            rest.len().checked_sub(digits + 1).map(|i| rest[i]),
            Some(b'+' | b'-')
        )
    }

    /// pdfTeX's `test_no_ligatures`: 1 unless a character of `f` has a
    /// ligature/kern program, or (as pdfTeX tests `odd(char_tag)`) an
    /// extensible recipe.
    fn test_no_ligatures(&self, f: i32) -> i32 {
        self.font_read(f, field::METRICS);
        let font = self.fonts.get(f);
        let any = (font.bc..=font.ec)
            .filter_map(|c| font.glyph(c))
            .any(|g| matches!(g.tag, Tag::Lig(_) | Tag::Ext(_)));
        i32::from(!any)
    }

    /// pdfTeX's `set_no_ligatures`.
    pub(crate) fn set_no_ligatures(&mut self, f: i32) {
        self.change_font(f);
        self.font_read(f, field::METRICS);
        self.fonts.retagged[fx(f)] = true;
        self.fonts.remade[fx(f)] = true;
        let font = self.fonts.metrics_mut(f);
        for c in font.bc..=font.ec {
            if let Some(g) = font.glyph_mut(c)
                && matches!(g.tag, Tag::Lig(_))
            {
                g.tag = Tag::None;
            }
        }
        self.font_wrote(f, field::METRICS);
    }

    /// pdfTeX's `get_tag_code`: 1, 2 or 4 for a ligature program, a
    /// larger character or an extensible recipe; -1 for a character the
    /// font lacks.
    fn tag_code(&self, f: i32, c: i32) -> i32 {
        self.font_read(f, field::METRICS);
        match self.fonts.get(f).glyph(c).map(|g| g.tag) {
            None => -1,
            Some(Tag::None) => 0,
            Some(Tag::Lig(_)) => 1,
            Some(Tag::List(_)) => 2,
            Some(Tag::Ext(_)) => 4,
        }
    }

    /// pdfTeX's `set_tag_code`: a negative value removes the tags of its
    /// bits (4 extensible, 2 list, 1 ligature).
    fn set_tag_code(&mut self, f: i32, c: i32, i: i32) {
        self.font_read(f, field::METRICS);
        self.fonts.retagged[fx(f)] = true;
        self.fonts.remade[fx(f)] = true;
        let bits = -i.clamp(-7, 0);
        if let Some(g) = self.fonts.metrics_mut(f).glyph_mut(c) {
            let remove = match g.tag {
                Tag::Ext(_) => bits & 4 != 0,
                Tag::List(_) => bits & 2 != 0,
                Tag::Lig(_) => bits & 1 != 0,
                Tag::None => false,
            };
            if remove {
                g.tag = Tag::None;
            }
        }
        self.font_wrote(f, field::METRICS);
    }
}

/// A character code `scan_char_num` gave.
fn byte(c: i32) -> u8 {
    u8::try_from(c).expect("a character code")
}
