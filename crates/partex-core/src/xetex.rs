//! `XeTeX`'s own commands (`XeTeX` §1460 and on): its `\convert`
//! functions, items of `\the`, and the errors that name a font's kind.

use crate::host::Host;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;

/// `XeTeX_revision`.
pub(crate) const XETEX_REVISION: &[u8] = b".999998";

impl<H: Host, T: Tracker> Tex<H, T> {
    /// `XeTeX` §1460–§1463: `\XeTeXrevision`, `\XeTeXvariationname`,
    /// `\XeTeXfeaturename`, `\XeTeXselectorname`, `\XeTeXglyphname`,
    /// `\Uchar` and `\Ucharcat` (`conv_toks`'s cases).
    pub(crate) fn xetex_conv_toks(&mut self, c: i32) -> Result<(), Jump> {
        let mut cat = 0;
        // Scan the argument for command `c`.
        match c {
            XETEX_REVISION_CODE => {}
            XETEX_VARIATION_NAME_CODE | XETEX_FEATURE_NAME_CODE | XETEX_SELECTOR_NAME_CODE => {
                // (AAT and Graphite fonts only: neither is made here)
                self.scan_font_ident()?;
                let fnt = self.cur_val;
                if c == XETEX_VARIATION_NAME_CODE {
                    self.font_kind_error(CONVERT, c, fnt, b"; not an AAT font")?;
                } else {
                    self.font_kind_error(CONVERT, c, fnt, b"; not an AAT or Graphite font")?;
                }
            }
            XETEX_GLYPH_NAME_CODE => {
                self.scan_font_ident()?;
                let fnt = self.cur_val;
                if self.is_native_font(fnt) {
                    return self.pdf_error(b"XeTeX", b"not implemented in partex yet");
                }
                self.font_kind_error(CONVERT, c, fnt, b"; not a native platform font")?;
            }
            XETEX_UCHAR_CODE => self.scan_usv_num()?,
            _ => {
                // `\Ucharcat`
                self.scan_usv_num()?;
                let saved_chr = self.cur_val;
                self.scan_int()?;
                let v = self.cur_val;
                if v < LEFT_BRACE || v > ACTIVE_CHAR || v == OUT_PARAM || v == IGNORE {
                    self.print_err(b"Invalid code (");
                    self.print_int(v);
                    self.print_str(b"), should be in the ranges 1..4, 6..8, 10..13");
                    self.help(&[b"I'm going to use 12 instead of that illegal code value."]);
                    self.error()?;
                    cat = 12;
                } else {
                    cat = v;
                }
                self.cur_val = saved_chr;
            }
        }
        let old_setting = self.selector();
        self.set_selector(crate::print::NEW_STRING);
        let b = self.pool_ptr;
        // Print the result of command `c`.
        match c {
            XETEX_REVISION_CODE => self.print_str(XETEX_REVISION),
            XETEX_UCHAR_CODE | XETEX_UCHARCAT_CODE => {
                self.print_char_x(crate::input::cu(self.cur_val));
            }
            _ => {}
        }
        self.set_selector(old_setting);
        let p = self.str_toks_cat(b, cat)?;
        self.ins_list(p)
    }

    /// `XeTeX` §1458: `not_aat_font_error` and its kin: "Cannot use", the
    /// command, "with", the font, and what the font is not.
    pub(crate) fn font_kind_error(
        &mut self,
        cmd: i32,
        c: i32,
        f: i32,
        not: &[u8],
    ) -> Result<(), Jump> {
        self.print_err(b"Cannot use ");
        self.print_cmd_chr(cmd, c);
        self.print_str(b" with ");
        self.print(self.fonts.name[crate::fonts::fx(f)]);
        self.print_str(not);
        self.error()
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// `XeTeX` §1450–§1457: `XeTeX`'s items of `last_item` (`\XeTeXversion`
    /// and the queries of native fonts; `\XeTeXglyphbounds`, a dimension).
    pub(crate) fn fetch_xetex_item(&mut self, m: i32) -> Result<(), Jump> {
        self.cur_val_level = INT_VAL;
        self.cur_val = match m {
            XETEX_VERSION_CODE => 0,
            XETEX_COUNT_VARIATIONS_CODE
            | XETEX_VARIATION_CODE
            | XETEX_VARIATION_MIN_CODE
            | XETEX_VARIATION_MAX_CODE
            | XETEX_VARIATION_DEFAULT_CODE => {
                self.scan_font_ident()?;
                0 // deprecated
            }
            XETEX_FONT_TYPE_CODE => {
                self.scan_font_ident()?;
                i32::from(self.is_native_font(self.cur_val)) * 2
            }
            XETEX_FIRST_CHAR_CODE | XETEX_LAST_CHAR_CODE if !self.native_query_ahead()? => {
                let n = self.cur_val;
                self.font_read(n, crate::track::font::METRICS);
                let font = self.fonts.get(n);
                if m == XETEX_FIRST_CHAR_CODE {
                    font.bc
                } else {
                    font.ec
                }
            }
            XETEX_COUNT_GLYPHS_CODE | XETEX_COUNT_FEATURES_CODE | XETEX_OT_COUNT_SCRIPTS_CODE
                if !self.native_query_ahead()? =>
            {
                0
            }
            _ => {
                return self.pdf_error(b"XeTeX", b"not implemented in partex yet");
            }
        };
        if m == XETEX_GLYPH_BOUNDS_CODE {
            self.cur_val_level = DIMEN_VAL;
        }
        Ok(())
    }

    /// Scan the font of a query of `\XeTeX...` items; whether it is a
    /// native font (its number in `cur_val`).
    fn native_query_ahead(&mut self) -> Result<bool, Jump> {
        self.scan_font_ident()?;
        Ok(self.is_native_font(self.cur_val))
    }
}
