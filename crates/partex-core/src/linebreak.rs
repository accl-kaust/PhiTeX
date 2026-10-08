//! Parts 38–39: Breaking paragraphs into lines (§813–§890).
//!
//! The engine's [`linebreak::line_break`] does the work; this module
//! gathers its parameters, prints what it reports, packs the lines and
//! appends them to the enclosing vertical list. `line_break` is a
//! recorded call (DESIGN 7.17.2), named by the paragraph's list, and the
//! lines' packing (§889) are its children, `hpack` calls.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem;

use partex_engine::expand::{self, ExpandEnv, Expansion};
use partex_engine::font::Font;
use partex_engine::hyph::Patterns;
use partex_engine::linebreak::{self, BreakKind, Broken, Event, Item};
use partex_engine::node::{BoxNode, FontId, GlueSpec, Node};
use partex_engine::pack::{Fonts, Spec};
use partex_ssa::Version;

use crate::arith::Scaled;
use crate::fonts::{Code, font_id};
use crate::host::Host;
use crate::pack::{Hpacked, Packer};
use crate::ssa::Func;
use crate::tex::{Jump, Tex};
use crate::track::{Row, Tracker, font as field, scalar};
use crate::web::*;

/// What line breaking reads besides its parameters (and where it loads
/// expanded fonts, holding on to what loading one raised).
struct BreakEnv<'a, H: Host, T: Tracker>(&'a mut Tex<H, T>, Option<Jump>);

impl<H: Host, T: Tracker> Fonts for BreakEnv<'_, H, T> {
    fn font(&self, f: FontId) -> &Font {
        self.0.font_read(i32::from(f.0), field::METRICS);
        self.0.fonts.font(f)
    }
}

impl<H: Host, T: Tracker> linebreak::Env for BreakEnv<'_, H, T> {
    fn cp_code(&self, f: FontId, code: u32, left: bool) -> i32 {
        let f = i32::from(f.0);
        // (a protrusion is in thousandths of the font's quad, a parameter)
        self.0.font_read(f, field::PARAMS);
        self.0.font_read(f, field::CODES + u32::from(!left));
        self.0.fonts.cp_code(f, code, !left)
    }
    fn lc_code(&self, c: u8) -> i32 {
        self.0.lc_code(i32::from(c))
    }
    fn hyphen_char(&self, f: FontId) -> i32 {
        self.0
            .tracker
            .read(crate::track::Cell::Font(i32::from(f.0)));
        self.0.font_read(i32::from(f.0), field::HYPHEN_CHAR);
        self.0.fonts.hyphen_char[usize::from(f.0)]
    }
    fn patterns(&self) -> &Patterns {
        self.0.patterns_read();
        &self.0.hyph.patterns
    }
    fn exception(&self, key: &alloc::vec::Vec<u8>) -> Option<&[u16]> {
        self.0.exception(key)
    }
    fn lc_code_usv(&self, c: i32) -> i32 {
        self.0.lc_code(c)
    }
    fn native_word(
        &mut self,
        font: FontId,
        actual_text: bool,
        text: &[u16],
    ) -> partex_engine::native::NativeWord {
        let mut w = partex_engine::native::NativeWord::new(font, actual_text, text.into());
        self.0.measure_native(&mut w);
        w
    }
    fn native_character(&mut self, font: FontId, c: i32) -> partex_engine::native::NativeWord {
        match self.0.new_native_character(i32::from(font.0), c) {
            Ok(w) => w,
            Err(j) => {
                // (a lost character's error stopped the job: line breaking
                // ends, and the jump is taken after it)
                self.1.get_or_insert(j);
                partex_engine::native::NativeWord::new(font, false, alloc::sync::Arc::from([]))
            }
        }
    }
}

impl<H: Host, T: Tracker> ExpandEnv for BreakEnv<'_, H, T> {
    fn expansion(&self, f: FontId) -> Expansion {
        self.0
            .tracker
            .read(crate::track::Cell::Font(i32::from(f.0)));
        self.0.font_read(i32::from(f.0), field::EXPAND);
        let x = self.0.fonts.expand[usize::from(f.0)];
        let font = |k: i32| (k != NULL_FONT).then(|| font_id(k));
        Expansion {
            step: x.step,
            stretch: font(x.stretch),
            shrink: font(x.shrink),
            ratio: x.ratio,
        }
    }
    fn ef_code(&self, f: FontId, c: u8) -> i32 {
        self.0.font_code(i32::from(f.0), Code::Ef, c)
    }
    fn margin_code(&self, f: FontId, c: u8, left: bool) -> i32 {
        let code = if left { Code::Lp } else { Code::Rp };
        // (a protrusion is in thousandths of the font's quad, a parameter)
        self.0.font_read(i32::from(f.0), field::PARAMS);
        self.0.font_code(i32::from(f.0), code, c)
    }
    fn origins(&mut self) -> Option<&mut partex_engine::origin::OrgTable> {
        self.0.org.as_deref_mut().map(|o| &mut o.table)
    }
    fn origin_table(&self) -> Option<&partex_engine::origin::OrgTable> {
        self.0.org.as_deref().map(|o| &o.table)
    }
    fn get_expand_font(&mut self, f: FontId, e: i32) -> FontId {
        if self.1.is_some() {
            return f;
        }
        match self.0.get_expand_font(i32::from(f.0), e) {
            Ok(k) => font_id(k),
            Err(j) => {
                self.1 = Some(j);
                f
            }
        }
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §815: break the current paragraph into lines, appending them to
    /// the enclosing vertical list; returns the last line (`just_box`).
    /// `display`: a display interrupts the paragraph. A recorded call
    /// (DESIGN 7.17.2), named by the paragraph's list and `display`; its
    /// result, `just_box`, is written to `scalar::LINE_BREAK_RESULT`.
    pub(crate) fn line_break(&mut self, display: bool) -> Result<Arc<BoxNode>, Jump> {
        if !T::VALUES {
            return self.line_break_body(display);
        }
        // (the list is the call's argument, named, not read)
        let list = self.cur_list.list.version();
        let name = Version::node(0x6c62_726b, &[list, Version::of(&display)]);
        self.tracker.call_begin(Func::LineBreak, &[name.0], self);
        let outer = self.nest.last().map(|l| l.list.len());
        if T::PURE {
            let reads = crate::fields::Reads {
                letters: true,
                marks: false,
            };
            let list = self.cur_list.list.to_vec();
            self.tracker.pure_fields(self.fields_version(&list, reads));
        }
        let r = self.line_break_body(display);
        if T::PURE {
            // (the contents of the lines it appended to the list around it)
            let lines: alloc::vec::Vec<u128> = self
                .cur_list
                .list
                .iter()
                .skip(outer.unwrap_or(0))
                .filter(|n| matches!(n, Node::Box(_)))
                .map(|n| Version::of(n).0)
                .collect();
            self.tracker.pure_lines(&lines);
        }
        if let Ok(b) = &r {
            self.tracker
                .row_wrote(Row::Scalar(scalar::LINE_BREAK_RESULT), b.parts_version());
        }
        self.tracker.call_end(self);
        r
    }

    fn line_break_body(&mut self, display: bool) -> Result<Arc<BoxNode>, Jump> {
        self.pack_begin_line = self.ml(); // this is for over/underfull box messages
        let list = mem::take(self.nodes_mut()).into_vec();
        let pg = self.pg();
        self.pop_nest();
        let params = self.break_params(display, pg);
        self.patterns_read();
        let broken = if self.hyph.trie_not_ready {
            // §891: INITEX packs the patterns when a paragraph first
            // needs them (the second pass).
            let b = self.break_lines(list.clone(), &params)?;
            if b.second_pass {
                self.init_trie()?;
                self.break_lines(list, &params)?
            } else {
                b
            }
        } else {
            self.break_lines(list, &params)?
        };
        // §827: TeX made `\leftskip` and `\rightskip` finite in place.
        if let Some(g) = broken.left_skip {
            self.set_glue_par_in_place(LEFT_SKIP_CODE, g);
        }
        if let Some(g) = broken.right_skip {
            self.set_glue_par_in_place(RIGHT_SKIP_CODE, g);
        }
        self.print_break_events(broken.events)?;
        // §877–§890: pack the lines and append them (sealed, `seal.rs`;
        // the last one is returned open).
        let mut just_box = None;
        self.seal_paragraph();
        let packer = if params.adjust_spacing > 0 {
            Packer::Expanded
        } else {
            Packer::Plain
        };
        let mut idx = 0;
        for item in broken.items {
            match item {
                Item::Line { list, width, shift } => {
                    // §889: call the packaging subroutine (it reports
                    // before the shift is set).
                    let mut migrated = Vec::new();
                    let packed = self.pack_line(list, width, &mut migrated, packer)?;
                    let mut b = packed.node;
                    // (`SyncTeX`: the line, made now, placed here)
                    self.sync_box(&mut b);
                    b.shift = shift;
                    let open = Arc::new(b.clone());
                    just_box = Some(open);
                    let b = self.seal_line(b, idx);
                    idx += 1;
                    // §888: append the new box to the current vertical
                    // list, preceded by the pre-adjustments (pdfTeX's and
                    // `XeTeX`'s `\vadjust pre`) and followed by the
                    // adjustments.
                    let (mut pre, mut post) = partex_engine::pack::split_migrated(migrated);
                    if !pre.is_empty() {
                        self.sync_list(&mut pre);
                        self.nodes_push().extend(pre);
                    }
                    self.append_to_vlist(Node::Box(b.share()));
                    if !post.is_empty() {
                        self.sync_list(&mut post);
                        self.nodes_push().extend(post);
                    }
                }
                Item::Penalty(pi) => self.nodes_push().push(Node::Penalty(pi)),
            }
        }
        self.set_pg(broken.prev_graf);
        *self.lr_save_mut() = broken.lr_open;
        self.pack_begin_line = 0;
        match just_box {
            Some(b) => Ok(b),
            None => self.confusion(b"line breaking"),
        }
    }

    /// §889: pack a line to `width`, its adjustment material migrating to
    /// `migrated`; with pdfTeX's `\pdfadjustspacing`, its fonts expanded to
    /// fit. An `hpack` call, a child of `line_break`'s.
    fn pack_line(
        &mut self,
        list: Vec<Node>,
        width: Scaled,
        migrated: &mut Vec<Node>,
        packer: Packer,
    ) -> Result<Hpacked, Jump> {
        let spec = Spec::Exactly(width);
        if packer != Packer::Expanded {
            return Ok(self.hpack_full(list, spec, Some(migrated)));
        }
        self.hpack_call(list, spec, Some(migrated), packer, |t, list, adjust| {
            let params = t.hpack_params();
            let mut env = BreakEnv(t, None);
            let packed = expand::hpack_line(list, width, &params, &mut env, adjust);
            if let Some(j) = env.1 {
                return Err(j);
            }
            Ok(t.hpacked(packed))
        })
    }

    fn break_lines(&mut self, list: Vec<Node>, params: &linebreak::Params) -> Result<Broken, Jump> {
        let mut env = BreakEnv(self, None);
        let broken = linebreak::line_break(list, params, &mut env);
        let jump = env.1;
        if let Some(j) = jump {
            return Err(j);
        }
        match broken {
            Ok(b) => Ok(b),
            Err(c) if linebreak::EXPANSION_ERRORS.contains(&c.0) => {
                self.pdf_error(b"font expansion", c.0.as_bytes())
            }
            Err(c) => self.confusion(c.0.as_bytes()),
        }
    }

    /// The parameters of breaking the paragraph whose `\prevgraf` field
    /// was `pg` into the current (enclosing) list.
    fn break_params(&self, display: bool, pg: i32) -> linebreak::Params {
        let (widow, widows) = if display {
            (DISPLAY_WIDOW_PENALTY_CODE, DISPLAY_WIDOW_PENALTIES_LOC)
        } else {
            (WIDOW_PENALTY_CODE, WIDOW_PENALTIES_LOC)
        };
        let final_widow_penalty = self.int_par(widow);
        let array = |loc| self.penalties(loc).map_or_else(Vec::new, <[i32]>::to_vec);
        linebreak::Params {
            inter_line_penalties: array(INTER_LINE_PENALTIES_LOC),
            club_penalties: array(CLUB_PENALTIES_LOC),
            widow_penalties: array(widows),
            pretolerance: self.int_par(PRETOLERANCE_CODE),
            tolerance: self.int_par(TOLERANCE_CODE),
            emergency_stretch: self.dimen_par(EMERGENCY_STRETCH_CODE),
            looseness: self.int_par(LOOSENESS_CODE),
            line_penalty: self.int_par(LINE_PENALTY_CODE),
            hyphen_penalty: self.int_par(HYPHEN_PENALTY_CODE),
            ex_hyphen_penalty: self.int_par(EX_HYPHEN_PENALTY_CODE),
            adj_demerits: self.int_par(ADJ_DEMERITS_CODE),
            double_hyphen_demerits: self.int_par(DOUBLE_HYPHEN_DEMERITS_CODE),
            final_hyphen_demerits: self.int_par(FINAL_HYPHEN_DEMERITS_CODE),
            inter_line_penalty: self.int_par(INTER_LINE_PENALTY_CODE),
            club_penalty: self.int_par(CLUB_PENALTY_CODE),
            broken_penalty: self.int_par(BROKEN_PENALTY_CODE),
            final_widow_penalty,
            left_skip: self.glue_par(LEFT_SKIP_CODE),
            right_skip: self.glue_par(RIGHT_SKIP_CODE),
            par_fill_skip: self.glue_par(PAR_FILL_SKIP_CODE),
            hsize: self.dimen_par(HSIZE_CODE),
            hang_indent: self.dimen_par(HANG_INDENT_CODE),
            hang_after: self.int_par(HANG_AFTER_CODE),
            par_shape: self.par_shape().map_or_else(Vec::new, |s| s.to_vec()),
            prev_graf: self.pg(),
            last_line_fit: self.int_par(LAST_LINE_FIT_CODE),
            lr_open: self.lr_save().clone(),
            language: pg % 0o200000,
            left_hyphen_min: pg / 0o20000000,
            right_hyphen_min: (pg / 0o200000) % 0o100,
            uc_hyph: self.int_par(UC_HYPH_CODE) > 0,
            max_hyphenatable_length: self.max_hyphenatable_length(),
            unicode: self.unicode,
            tracing: self.int_par(TRACING_PARAGRAPHS_CODE) > 0,
            // (`XeTeX`'s is `\XeTeXprotrudechars`)
            protrude_chars: self.int_par(if self.unicode {
                XETEX_PROTRUDE_CHARS_CODE
            } else {
                PDF_PROTRUDE_CHARS_CODE
            }),
            adjust_spacing: self.int_par(PDF_ADJUST_SPACING_CODE),
            pack: self.hpack_params(),
            pdftex_bugs: self.params.pdftex_bugs,
        }
    }

    /// Replace glue parameter `n`'s value without the save stack.
    fn set_glue_par_in_place(&mut self, n: i32, g: GlueSpec) {
        let g = crate::objs::Obj::Glue(self.new_glue_value(g, None));
        let w = self.peek_eqtb(GLUE_BASE + n);
        self.set_eqtb_entry(GLUE_BASE + n, w, Some(g));
    }

    /// Print what line breaking reported, in order.
    fn print_break_events(&mut self, events: Vec<Event>) -> Result<(), Jump> {
        let tracing = self.int_par(TRACING_PARAGRAPHS_CODE) > 0;
        for e in events {
            match e {
                Event::Begin { first } => {
                    // §863
                    self.begin_diagnostic();
                    if first {
                        self.print_nl(b"@firstpass");
                    }
                    self.font_in_short_display = NULL_FONT; // §864
                }
                Event::SecondPass => {
                    self.print_nl(b"@secondpass");
                    self.font_in_short_display = NULL_FONT;
                }
                Event::EmergencyPass => {
                    self.print_nl(b"@emergencypass");
                    self.font_in_short_display = NULL_FONT;
                }
                Event::Text(nodes) => {
                    // §857
                    self.print_nl(b"");
                    self.short_display(&nodes);
                }
                Event::Feasible {
                    at,
                    via,
                    badness,
                    penalty,
                    demerits,
                } => {
                    // §856: print a symbolic description of this feasible
                    // break.
                    self.print_nl(b"@");
                    match at {
                        BreakKind::Par => self.print_esc(b"par"),
                        BreakKind::Glue => {}
                        BreakKind::Penalty => self.print_esc(b"penalty"),
                        BreakKind::Discretionary => self.print_esc(b"discretionary"),
                        BreakKind::Kern => self.print_esc(b"kern"),
                        BreakKind::Math => self.print_esc(b"math"),
                    }
                    self.print_str(b" via @@");
                    self.print_int(via);
                    self.print_str(b" b=");
                    match badness {
                        Some(b) => self.print_int(b),
                        None => self.print_char(b'*'),
                    }
                    self.print_str(b" p=");
                    self.print_int(penalty);
                    self.print_str(b" d=");
                    match demerits {
                        Some(d) => self.print_int(d),
                        None => self.print_char(b'*'),
                    }
                }
                Event::NewBreak {
                    serial,
                    line,
                    fitness,
                    hyphenated,
                    total,
                    previous,
                    last_fit,
                } => {
                    // §846: print a symbolic description of the new break
                    // node.
                    self.print_nl(b"@@");
                    self.print_int(serial);
                    self.print_str(b": line ");
                    self.print_int(line);
                    self.print_char(b'.');
                    self.print_int(i32::from(fitness));
                    if hyphenated {
                        self.print_char(b'-');
                    }
                    self.print_str(b" t=");
                    self.print_int(total);
                    if let Some((short, glue, at_end)) = last_fit {
                        // e-TeX: print additional data in the new active node.
                        self.print_str(b" s=");
                        self.print_scaled(short);
                        self.print_str(if at_end { b" a=" } else { b" g=" });
                        self.print_scaled(glue);
                    }
                    self.print_str(b" -> @@");
                    self.print_int(previous);
                }
                Event::End => {
                    self.end_diagnostic(true);
                    self.normalize_selector()?;
                }
                Event::InfiniteShrink => {
                    // §826
                    if tracing {
                        self.end_diagnostic(true);
                    }
                    self.print_err(b"Infinite glue shrinkage found in a paragraph");
                    self.help(&[
                        b"The paragraph just ended includes some glue that has",
                        b"infinite shrinkability, e.g., `\\hskip 0pt minus 1fil'.",
                        b"Such glue doesn't belong there---it allows a paragraph",
                        b"of any length to fit on one line. But it's safe to proceed,",
                        b"since the offensive shrinkability has been made finite.",
                    ]);
                    self.error()?;
                    if tracing {
                        self.begin_diagnostic();
                    }
                }
                Event::MissingChar { font, ch } => self.char_warning(i32::from(font.0), ch)?,
            }
        }
        Ok(())
    }
}
