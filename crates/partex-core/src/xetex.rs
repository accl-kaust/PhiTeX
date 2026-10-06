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
        let mut glyph = None;
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
                if let Some(nf) = self.native_font(fnt).cloned() {
                    self.scan_int()?;
                    glyph = Some(nf.font.glyph_name(u32::try_from(self.cur_val).unwrap_or(0)));
                } else {
                    self.font_kind_error(CONVERT, c, fnt, b"; not a native platform font")?;
                }
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
            XETEX_GLYPH_NAME_CODE => {
                // `print_glyph_name`
                for &k in glyph.iter().flatten() {
                    self.print_char(k);
                }
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
        use partex_otf::xetex::what;
        const NOT_NATIVE: &[u8] = b"; not a native platform font";
        const NOT_OT: &[u8] = b"; not an OpenType Layout font";
        const NOT_AAT_GR: &[u8] = b"; not an AAT or Graphite font";
        self.cur_val_level = INT_VAL;
        // (no AAT or Graphite font is made here: a native font is an
        // OpenType one, `is_ot_font`)
        let ot = |t: &Self, n: i32| t.native_font(n).cloned();
        self.cur_val = match m {
            XETEX_VERSION_CODE => 0,
            XETEX_COUNT_VARIATIONS_CODE
            | XETEX_VARIATION_CODE
            | XETEX_VARIATION_MIN_CODE
            | XETEX_VARIATION_MAX_CODE
            | XETEX_VARIATION_DEFAULT_CODE
            | XETEX_COUNT_FEATURES_CODE => {
                // (deprecated; features count in AAT and Graphite fonts)
                self.scan_font_ident()?;
                0
            }
            XETEX_COUNT_GLYPHS_CODE | XETEX_OT_COUNT_SCRIPTS_CODE => {
                self.scan_font_ident()?;
                ot(self, self.cur_val).map_or(0, |n| n.font.ot_font_get(m - XETEX_INT))
            }
            XETEX_FEATURE_CODE_CODE
            | XETEX_IS_EXCLUSIVE_FEATURE_CODE
            | XETEX_COUNT_SELECTORS_CODE
            | XETEX_SELECTOR_CODE_CODE
            | XETEX_IS_DEFAULT_SELECTOR_CODE
            | XETEX_FIND_FEATURE_BY_NAME_CODE
            | XETEX_FIND_SELECTOR_BY_NAME_CODE => {
                self.scan_font_ident()?;
                let n = self.cur_val;
                self.font_kind_error(LAST_ITEM, m, n, NOT_AAT_GR)?;
                -1
            }
            XETEX_FIND_VARIATION_BY_NAME_CODE => {
                self.scan_font_ident()?;
                let n = self.cur_val;
                self.font_kind_error(LAST_ITEM, m, n, b"; not an AAT font")?;
                -1
            }
            XETEX_OT_COUNT_LANGUAGES_CODE
            | XETEX_OT_SCRIPT_CODE
            | XETEX_OT_COUNT_FEATURES_CODE
            | XETEX_OT_LANGUAGE_CODE
            | XETEX_OT_FEATURE_CODE => {
                self.scan_font_ident()?;
                let n = self.cur_val;
                match ot(self, n) {
                    None => {
                        self.font_kind_error(LAST_ITEM, m, n, NOT_OT)?;
                        -1
                    }
                    Some(f) => {
                        self.scan_int()?;
                        let k = self.cur_val;
                        let what = m - XETEX_INT;
                        match what {
                            what::OT_COUNT_LANGUAGES | what::OT_SCRIPT_CODE => {
                                f.font.ot_font_get1(what, k)
                            }
                            what::OT_COUNT_FEATURES | what::OT_LANGUAGE_CODE => {
                                self.scan_int()?;
                                f.font.ot_font_get2(what, k, self.cur_val)
                            }
                            _ => {
                                self.scan_int()?;
                                let kk = self.cur_val;
                                self.scan_int()?;
                                f.font.ot_font_get3(what, k, kk, self.cur_val)
                            }
                        }
                    }
                }
            }
            XETEX_MAP_CHAR_TO_GLYPH_CODE | XETEX_GLYPH_INDEX_CODE => {
                let f = self.cur_font();
                match ot(self, f) {
                    None => {
                        self.font_kind_error(LAST_ITEM, m, f, NOT_NATIVE)?;
                        0
                    }
                    Some(nf) if m == XETEX_MAP_CHAR_TO_GLYPH_CODE => {
                        self.scan_int()?;
                        nf.font.map_char_to_glyph(self.cur_val)
                    }
                    Some(nf) => {
                        // `scan_and_pack_name`
                        self.scan_file_name()?;
                        self.pack_file_name(self.cur_name, self.cur_area, self.cur_ext);
                        nf.font.map_glyph_to_index(&self.name_of_file)
                    }
                }
            }
            XETEX_FONT_TYPE_CODE => {
                self.scan_font_ident()?;
                i32::from(self.is_native_font(self.cur_val)) * 2
            }
            XETEX_FIRST_CHAR_CODE | XETEX_LAST_CHAR_CODE => {
                self.scan_font_ident()?;
                let n = self.cur_val;
                if let Some(nf) = ot(self, n) {
                    nf.font.font_char_range(m == XETEX_FIRST_CHAR_CODE)
                } else {
                    self.font_read(n, crate::track::font::METRICS);
                    let font = self.fonts.get(n);
                    if m == XETEX_FIRST_CHAR_CODE {
                        font.bc
                    } else {
                        font.ec
                    }
                }
            }
            XETEX_GLYPH_BOUNDS_CODE => {
                self.cur_val_level = DIMEN_VAL;
                let f = self.cur_font();
                match ot(self, f) {
                    None => {
                        self.font_kind_error(LAST_ITEM, m, f, NOT_NATIVE)?;
                        0
                    }
                    Some(nf) => {
                        self.scan_int()?;
                        let n = self.cur_val; // which edge: 1=left, 2=top, 3=right, 4=bottom
                        if (1..=4).contains(&n) {
                            self.scan_int()?; // glyph number
                            nf.font
                                .glyph_bounds_edge(n, u32::try_from(self.cur_val).unwrap_or(0))
                        } else {
                            self.print_err(
                                b"\\\\XeTeXglyphbounds requires an edge index from 1 to 4;",
                            );
                            self.print_nl(b"I don't know anything about edge ");
                            self.print_int(n);
                            self.error()?;
                            0
                        }
                    }
                }
            }
            _ => self.pdf_page_count()?, // `\XeTeXpdfpagecount`
        };
        Ok(())
    }

    /// e-TeX's `\fontcharwd` and kin of native font `f` (`getnativecharwd`
    /// …): `None` if `f` is not one.
    pub(crate) fn native_char_dimen(&self, m: i32, f: i32, c: i32) -> Option<i32> {
        let nf = self.native_font(f)?;
        let font = self.fonts.get(f);
        let p = |k: i32| font.param(crate::input::ux(k));
        Some(match m {
            FONT_CHAR_WD_CODE => nf.font.char_wd(c),
            FONT_CHAR_HT_CODE | FONT_CHAR_DP_CODE => {
                let (h, d) = nf
                    .font
                    .char_height_depth(c, p(QUAD_CODE), p(X_HEIGHT_CODE), p(8));
                if m == FONT_CHAR_HT_CODE { h } else { d }
            }
            _ => nf.font.char_ic(c, nf.font.letter_space),
        })
    }

    /// Scan the font of a query of `\XeTeX...` items; whether it is a
    /// native font (its number in `cur_val`).
    fn native_query_ahead(&mut self) -> Result<bool, Jump> {
        self.scan_font_ident()?;
        Ok(self.is_native_font(self.cur_val))
    }
}
