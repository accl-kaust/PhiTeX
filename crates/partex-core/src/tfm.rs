//! Part 30: Font metric data (§539–§582): loading TFM files (parsed by
//! the engine's [`Font`]) and `MLTeX`'s `effective_char` (merged source).

use alloc::sync::Arc;

use partex_engine::font::{Font, Glyph};
use partex_engine::node::{Glyphs, Node};

use crate::arith::Scaled;
use crate::fonts::{font_id, fx};
use crate::host::{FileKind, Host};
use crate::input::ux;
use crate::mem::Pointer;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::track::font as field;
use crate::web::*;

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §554: does character `c` exist in font `f`? (A read of its
    /// metrics.)
    #[inline]
    pub(crate) fn char_exists(&self, f: i32, c: i32) -> bool {
        self.font_read(f, field::METRICS);
        self.fonts.get(f).glyph(c).is_some()
    }

    /// §554: the metrics `char_info(f)(c)` refers to (through `MLTeX`'s
    /// `effective_char`, which may report a missing substitution); zero
    /// for a missing character.
    pub(crate) fn char_metrics(&mut self, f: i32, c: i32) -> Glyph {
        let e = self.effective_char(true, f, c);
        self.font_read(f, field::METRICS);
        self.fonts.get(f).glyph(e).unwrap_or_default()
    }

    /// merged source: `MLTeX`'s `char_list_exists`, `char_list_char`.
    fn char_sub_code(&self, c: i32) -> i32 {
        self.equiv(CHAR_SUB_CODE_BASE + c)
    }

    /// merged source: `MLTeX`'s `effective_char`: `c`, or the base
    /// character of its substitution when `c` is missing from `f`.
    pub(crate) fn effective_char(&mut self, err_p: bool, f: i32, c: i32) -> i32 {
        if !self.mltex_enabled_p {
            return c;
        }
        let fi = fx(f);
        if self.char_exists(f, c) {
            return c;
        }
        if c >= self.int_par(CHAR_SUB_DEF_MIN_CODE)
            && c <= self.int_par(CHAR_SUB_DEF_MAX_CODE)
            && self.char_sub_code(c) > 0
        {
            let base_c = self.char_sub_code(c) % 256;
            if !err_p {
                return base_c;
            }
            if self.char_exists(f, base_c) {
                return base_c;
            }
        }
        if err_p {
            // print error and return existing character
            self.begin_diagnostic();
            self.print_nl(b"Missing character: There is no ");
            self.print_str(b"substitution for ");
            self.print(c);
            self.print_str(b" in font ");
            self.slow_print(self.fonts.name[fi]);
            self.print_char(b'!');
            self.end_diagnostic(false);
            return self.fonts.get(f).bc; // N.B.: not non-existing character `c`!
        }
        c
    }

    /// §560: input a TFM file for `\font u=nom at s` (`s` < 0 means
    /// `scaled -s`, -1000 the design size). Returns the font or `null_font`.
    pub(crate) fn read_font_info(
        &mut self,
        u: Pointer,
        nom: i32,
        aire: i32,
        s: Scaled,
    ) -> Result<i32, Jump> {
        // §563: open `tfm_file` for input.
        let name_too_long = self.length(ux(nom)) > 255 || self.length(ux(aire)) > 255;
        let mut file_opened = false;
        if !name_too_long {
            let empty = self.pool_str(b"");
            self.pack_file_name(nom, aire, empty);
            let name = self.name_of_file.clone();
            let found = self.host.read_file(&name, FileKind::Tfm);
            if T::VALUES {
                // (a load, found or not: a metric file that comes after
                // the font failed wakes the step that asked for it)
                self.tracker
                    .load(&name, FileKind::Tfm, found.as_ref().map(|f| &f.contents));
            }
            if let Some(file) = found {
                file_opened = true;
                let data: Arc<[u8]> = file.contents;
                if let Some(g) = self.read_tfm(&data, u, nom, aire, s) {
                    return g;
                }
            }
        }
        // §561: report that the font won't be loaded.
        self.start_font_error_message(u, nom, aire, s);
        if file_opened {
            self.print_str(b" not loadable: Bad metric (TFM) file");
        } else if name_too_long {
            self.print_str(b" not loadable: Metric (TFM) file name too long");
        } else {
            self.print_str(b" not loadable: Metric (TFM) file not found");
        }
        self.help(&[
            b"I wasn't able to read the size data for this font,",
            b"so I will ignore the font specification.",
            b"[Wizards can fix TFM files using TFtoPL/PLtoTF.]",
            b"You might try inserting a different font spec;",
            b"e.g., type `I\\font<same font id>=<substitute font name>'.",
        ]);
        self.error()?;
        Ok(NULL_FONT)
    }

    /// §562–§576: read and check the font data; `None` is tex.web's
    /// `abort` (a bad TFM file). The checks happen in tex.web's order:
    /// the size fields, the room for the font, the header, the scaling
    /// (whose error does not stop the loading), then the rest.
    fn read_tfm(
        &mut self,
        data: &Arc<[u8]>,
        u: Pointer,
        nom: i32,
        aire: i32,
        s: Scaled,
    ) -> Option<Result<i32, Jump>> {
        // §565–§566
        let lf = Font::memory_words(data).ok()?;
        // (the room left is the table's: `font_ptr`, `fmem_ptr`)
        self.font_table_read();
        if self.font_ptr == self.params.font_max || self.fmem_ptr + lf > self.params.font_mem_size {
            // §567: apologize for not loading the font, `goto done`.
            self.start_font_error_message(u, nom, aire, s);
            self.print_str(b" not loaded: Not enough room left");
            self.help(&[
                b"I'm afraid I won't be able to make use of this font,",
                b"because my memory for character-size data is too small.",
                b"If you're really stuck, ask a wizard to enlarge me.",
                b"Or maybe try `I\\font<same font id>=<name of loaded font>'.",
            ]);
            return Some(self.error().map(|()| NULL_FONT));
        }
        // §568: the design size, and the size to load at.
        let mut z = Font::design_size_of(data).ok()?;
        if s != -1000 {
            if s >= 0 {
                z = s;
            } else {
                self.save_arith_error = self.arith_error;
                let sw = z;
                z = self.xn_over_d(z, -s, 1000);
                if self.arith_error || z >= 0o1000000000 {
                    self.start_font_error_message(u, nom, aire, s);
                    self.print_str(b" scaled to 2048pt or higher");
                    self.help(&[b"I will ignore the scaling factor."]);
                    if let Err(j) = self.error() {
                        return Some(Err(j));
                    }
                    z = sw;
                }
                self.arith_error = self.save_arith_error;
            }
        }
        let font = Font::from_tfm(data, Some(z)).ok()?;
        // §576: make final adjustments and `goto done`.
        let (name, area) = (
            self.str_bytes(ux(nom)).to_vec(),
            self.str_bytes(ux(aire)).to_vec(),
        );
        let (f, ident, idv) = self.new_font_slot(
            &crate::fonts::FontIdent::Tfm {
                tfm: data,
                name: &name,
                area: &area,
                size: z,
            },
            &name,
        );
        self.tracker.read(crate::track::Cell::FontTable);
        self.tracker.write(crate::track::Cell::FontTable);
        self.tracker.write(crate::track::Cell::Font(f));
        self.fonts
            .place(f, font, data.clone(), nom, aire, ident, idv);
        self.font_named(nom);
        self.font_loaded(f);
        self.fonts.hyphen_char[fx(f)] = self.int_par(DEFAULT_HYPHEN_CHAR_CODE);
        self.fonts.skew_char[fx(f)] = self.int_par(DEFAULT_SKEW_CHAR_CODE);
        self.fmem_ptr += lf;
        self.font_ptr += 1;
        // (the font is a value, and so is each of its fields; the table
        // has it now)
        self.font_made(f);
        Some(Ok(f))
    }

    /// Font `f` made again in its slot, as §1260's search found it where
    /// the program has not made it (a rebuild's step run again: its own
    /// older run's font, or a later step's): its parameters the TFM's
    /// again, the font memory they took past them given back, its hyphen
    /// and skew characters the defaults (§576), and the newest font
    /// ([`crate::track::Tracker::font_newest`]), as a load here would
    /// leave it. An `\intarray` (a font whose parameters grow) made again
    /// in a trip finds it so.
    pub(crate) fn remake_font(&mut self, f: i32) {
        let data = self.fonts.tfm[fx(f)].clone();
        let size = self.fonts.get(f).size;
        if let Ok(font) = Font::from_tfm(&data, Some(size)) {
            let had = self.fonts.params_mut(f).len();
            let n = font.params.len();
            *self.fonts.params_mut(f) = font.params;
            self.fmem_ptr = self
                .fmem_ptr
                .saturating_sub(i32::try_from(had.saturating_sub(n)).unwrap_or(0));
        }
        self.tracker.write(crate::track::Cell::FontTable);
        self.tracker.write(crate::track::Cell::Font(f));
        self.fonts.hyphen_char[fx(f)] = self.int_par(DEFAULT_HYPHEN_CHAR_CODE);
        self.fonts.skew_char[fx(f)] = self.int_par(DEFAULT_SKEW_CHAR_CODE);
        self.font_made(f);
        // (loaded here in program order: its number is the link's)
        self.font_loaded(f);
    }

    /// §561: `start_font_error_message`.
    fn start_font_error_message(&mut self, u: Pointer, nom: i32, aire: i32, s: Scaled) {
        self.print_err(b"Font ");
        self.sprint_cs(u);
        self.print_char(b'=');
        let empty = self.pool_str(b"");
        self.print_file_name(nom, aire, empty);
        if s >= 0 {
            self.print_str(b" at ");
            self.print_scaled(s);
            self.print_str(b"pt");
        } else if s != -1000 {
            self.print_str(b" scaled ");
            self.print_int(-s);
        }
    }

    /// §581: warn about a missing character.
    pub(crate) fn char_warning(&mut self, f: i32, c: i32) -> Result<(), Jump> {
        // (the warning prints the font's name, a part of its metrics)
        self.font_read(f, field::METRICS);
        if self.params.flavor == crate::params::Flavor::PdfTex {
            return self.pdftex_char_warning(f, c);
        }
        if self.int_par(TRACING_LOST_CHARS_CODE) > 0 {
            self.begin_diagnostic();
            self.print_nl(b"Missing character: There is no ");
            self.print(c);
            self.print_str(b" in font ");
            self.slow_print(self.fonts.name[fx(f)]);
            self.print_char(b'!');
            self.end_diagnostic(false);
        }
        Ok(())
    }

    /// pdfTeX's `char_warning`: `\tracinglostchars>1` shows the warning
    /// on the terminal too (in extended mode), `>2` makes it an error.
    fn pdftex_char_warning(&mut self, f: i32, c: i32) -> Result<(), Jump> {
        let lost = self.int_par(TRACING_LOST_CHARS_CODE);
        if lost <= 0 {
            return Ok(());
        }
        let old_setting = self.int_par(TRACING_ONLINE_CODE);
        if self.etex_ex() && lost > 1 {
            self.set_int_par(TRACING_ONLINE_CODE, 1);
        }
        if lost > 2 {
            self.print_err(b"Missing character: There is no ");
        } else {
            self.begin_diagnostic();
            self.print_nl(b"Missing character: There is no ");
        }
        self.print(c);
        if lost > 2 {
            self.print_str(b" (");
            self.print_hex(c);
            self.print_str(b")");
        }
        self.print_str(b" in font ");
        self.slow_print(self.fonts.name[fx(f)]);
        if lost < 3 {
            self.print_char(b'!');
        }
        self.set_int_par(TRACING_ONLINE_CODE, old_setting);
        if lost > 2 {
            self.help(&[]);
            self.error()
        } else {
            self.end_diagnostic(false);
            Ok(())
        }
    }

    /// §582: a character node for `c` in font `f`, or `None` (after a
    /// warning).
    pub(crate) fn new_character(&mut self, f: i32, c: i32) -> Result<Option<Node>, Jump> {
        let ec = self.effective_char(false, f, c);
        if self.char_exists(f, ec) {
            // N.B.: not `char_info`
            let Ok(c) = u8::try_from(c) else {
                return Ok(None);
            };
            return Ok(Some(Node::Glyphs(Glyphs::one(font_id(f), c))));
        }
        self.char_warning(f, c)?;
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{engine, term_output};

    /// Oracle: `\font\x=cmr10 \font\y=cmr10 scaled 1200 \font\z=cmr10 at
    /// 7.3pt` and `\the\fontdimen n` of each, in INITEX.
    #[test]
    fn loads_cmr10_like_tex() {
        let mut t = engine();
        t.init_prim().unwrap();
        t.init_null_font();
        t.host.files.insert(
            b"cmr10.tfm".to_vec(),
            include_bytes!("../testdata/cmr10.tfm").to_vec(),
        );
        let nom = t.make_tex_string(b"cmr10").unwrap();
        let aire = t.pool_str(b"");
        let x = t.read_font_info(0, nom, aire, -1000).unwrap();
        let y = t.read_font_info(0, nom, aire, -1200).unwrap();
        let z = t.read_font_info(0, nom, aire, 7 * 65536 + 19661).unwrap();
        assert_eq!((x, y, z), (1, 2, 3));
        let mut dimens = |params: &[(i32, i32)]| {
            t.term_offset = 0;
            let out = term_output(&mut t, |t| {
                for &(n, f) in params {
                    t.print_scaled(t.font_param(n, f));
                    t.print_char(b',');
                }
            });
            alloc::string::String::from_utf8(out).unwrap()
        };
        let all: alloc::vec::Vec<_> = (1..=7).map(|n| (n, x)).collect();
        assert_eq!(
            dimens(&all),
            "0.0,3.33333,1.66666,1.11111,4.30554,10.00002,1.11111,"
        );
        assert_eq!(
            dimens(&[(2, y), (6, y), (2, z), (6, z)]),
            "4.0,12.00003,2.43333,7.30002,"
        );
        // `A` exists in cmr10; character 200 does not.
        assert!(t.new_character(x, 65).unwrap().is_some());
        assert!(t.new_character(x, 200).unwrap().is_none());
    }
}
