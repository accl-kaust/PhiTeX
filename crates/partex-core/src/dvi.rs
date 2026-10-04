//! Part 32: Shipping pages out (§616–§643), engine side, with `MLTeX`'s
//! character substitution and the whatsit output of part 53
//! (§1366–§1378).
//!
//! `ship_out` walks the box once, in tex.web's `hlist_out`/`vlist_out`
//! order, and turns it into an owned [`Page`](crate::pageir::Page): that
//! walk performs `\write`, `\openout` and `\closeout`, expands `\special`
//! texts and sets glue. The DVI bytes are then produced by the
//! [`DviWriter`] backend, which never looks at engine state.

use alloc::vec::Vec;

use crate::arith::{Scaled, zround};
use crate::dviout::{DviWriter, TooLong, page_bound};
use crate::fonts::fx;
use crate::host::{FileKind, Host};
use crate::input::ux;
use partex_engine::node::{
    BoxNode, GlueSign, GlueSpec, LeaderNode, Leaders, Node, RUNNING, Tokens, Whatsit,
};

use crate::pageir::{FontDef, Item, LeaderKind, Page};
use crate::pdf::val::{DVI_ALL, Record, Val};
use crate::print::{LOG_ONLY, NEW_STRING, TERM_AND_LOG};
use crate::scan::MAX_DIMEN;
use crate::tex::{Jump, Tex};
use crate::track::{Output, Tracker};
use crate::web::*;
use partex_engine::lr;

/// §625: `billion`.
const BILLION: f64 = 1_000_000_000.0;

/// §625: `vet_glue`.
fn vet_glue(g: f64) -> f64 {
    g.clamp(-BILLION, BILLION)
}

/// The engine's view of the DVI file: DESIGN 7.17.12's `dvi` row, whose
/// tables are values `ship_out` writes (`pdf::val::dvi_field`): the
/// file's state (`FILE`), and the writer's fonts defined, totals and
/// place in the file, the parts of one shared value.
/// The DVI writer's `FILE` field ([`DviState::file_part`]).
pub(crate) type FilePart = (bool, bool, u64, Option<crate::host::WriteId>, bool);

#[derive(Clone, Default)]
pub(crate) struct DviState {
    /// A page has been shipped out: the preamble is written.
    started: bool,
    /// The backend, from the first page shipped out on, unless the host's
    /// [`PageSink`](crate::host::PageSink) has it (`away`).
    pub(crate) writer: Option<Val<DviWriter>>,
    away: bool,
    /// While `away`: an upper bound on the file's length.
    bound: u64,
    pub(crate) file: Option<crate::host::WriteId>,
    /// The file outgrew 2^31 bytes (§598's `cur_s:=-2`).
    pub(crate) too_long: bool,
    /// The page being built (its buffers are reused from page to page).
    page: Page,
    /// Which fonts the current page lists, by internal number.
    page_fonts: Vec<bool>,
}

partex_engine::persist_struct!(DviState {
    started,
    writer,
    away,
    bound,
    file,
    too_long,
    page,
    page_fonts
});

/// The writer's parts: the fonts defined, the totals (`total_pages`,
/// `max_v`, `max_h`, `max_push`), and its place in the file with what it
/// holds back.
impl Record for DviWriter {
    const PARTS: usize = 3;
    fn part_version(&self, part: usize) -> u128 {
        DviWriter::part_version(self, part)
    }
}

impl DviState {
    /// The `FILE` field's version: the preamble written, whether the page
    /// sink has the file, the bound on its length, the file, too long.
    /// The `FILE` field's value: the preamble written, whether the host's
    /// sink has the writer, its length bound, the file, too long.
    pub(crate) fn file_part(&self) -> FilePart {
        (
            self.started,
            self.away,
            self.bound,
            self.file,
            self.too_long,
        )
    }

    /// The `FILE` field stored back.
    pub(crate) fn set_file_part(&mut self, v: &FilePart) {
        (
            self.started,
            self.away,
            self.bound,
            self.file,
            self.too_long,
        ) = *v;
    }

    pub(crate) fn file_version(&self) -> u128 {
        partex_ssa::Version::of(&(
            self.started,
            self.away,
            self.bound,
            self.file.map(|f| f.0),
            self.too_long,
        ))
        .0
    }

    /// Hash what the rest of the output depends on (not the scratch page),
    /// with where in the file it is and the bytes not written yet if
    /// `placed`: outputs are effects that a runtime reuses as they are
    /// (machine mode), where a checkpoint session relocates them.
    pub(crate) fn hash_state<S: core::hash::Hasher>(&self, h: &mut S, placed: bool) {
        use core::hash::Hash;
        (
            self.started,
            self.away,
            self.too_long,
            self.file.map(|f| f.0),
        )
            .hash(h);
        match &self.writer {
            Some(w) => {
                1u8.hash(h);
                w.hash_state(h);
                if placed {
                    w.hash_placement(h);
                }
            }
            None => 0u8.hash(h),
        }
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Font `f`'s number in the DVI file (§576, §621): as the program made
    /// it, a read of the font's `NUMBER` field, with a tracker that keeps
    /// values (an SSA build's table keeps fonts only an older run made);
    /// else, and for a font made with the table, its place in the table
    /// (`FontArrays::number`).
    pub(crate) fn dvi_font_number(&self, f: i32) -> i32 {
        if T::VALUES {
            let n = self
                .fonts
                .num
                .get(crate::fonts::fx(f))
                .copied()
                .unwrap_or(0);
            if n > 0 {
                self.font_read(f, crate::track::font::NUMBER);
                return n;
            }
        }
        self.fonts.number(f)
    }

    /// §532: `ensure_dvi_open`.
    fn ensure_dvi_open(&mut self) -> Result<(), Jump> {
        if self.output_file_name() == 0 {
            if self.job_name() == 0 {
                self.open_log_file()?;
            }
            self.pack_job_name(b".dvi");
            loop {
                let name = self.name_of_file.clone();
                if let Some((id, printed)) = self.open_out(&name, FileKind::Other) {
                    self.dvi.file = Some(id);
                    self.name_of_file = printed;
                    break;
                }
                self.prompt_file_name(b"file name for output", b".dvi")?;
            }
            let s = self.make_name_string()?;
            self.set_output_file_name(s);
        }
        Ok(())
    }

    /// Bytes to the DVI file as an effect emitted again (a hit's).
    pub(crate) fn dvi_bytes_raw(&mut self, bytes: &[u8]) {
        if let Some(id) = self.dvi.file {
            self.out_write(id, bytes);
        }
    }

    /// A page sink's effect emitted again (a hit's): the writer handed
    /// over, a page queued or written at once, or the file finished, as
    /// the body did (`page_effect`).
    pub(crate) fn page_sink_raw(&mut self, what: Output, bytes: &[u8]) {
        use partex_engine::persist::{Loader, Persist};
        let file = self.dvi.file;
        let Some(sink) = self.host.page_sink() else {
            return;
        };
        let mut l = Loader::new(bytes);
        match what {
            Output::DviStart => {
                if let (Some(w), Some(file)) = (DviWriter::load(&mut l), file) {
                    sink.start(w, file);
                }
            }
            Output::DviPage => {
                if let Some(p) = Page::load(&mut l) {
                    sink.page(p);
                }
            }
            Output::DviPageNow => {
                if let Some(p) = Page::load(&mut l) {
                    let _ = sink.page_now(p);
                }
            }
            Output::DviFinish => {
                if let Ok(mag) = <[u8; 4]>::try_from(bytes) {
                    let _ = sink.finish(i32::from_le_bytes(mag));
                }
            }
            _ => {}
        }
    }

    /// Hand the backend's finished bytes to the file (an effect).
    fn flush_dvi(&mut self) {
        if let (Some(w), Some(id)) = (self.dvi.writer.as_mut(), self.dvi.file) {
            let bytes = w.drain();
            if !bytes.is_empty() {
                if T::VALUES {
                    self.tracker.output(Output::Dvi, &bytes);
                }
                self.out_write(id, &bytes);
            }
        }
    }

    /// A page given to the host's page sink: an effect, the page as
    /// saved.
    fn page_effect(&mut self, what: Output, page: &Page) {
        if T::VALUES {
            let mut s = partex_engine::persist::Saver::default();
            partex_engine::persist::Persist::save(page, &mut s);
            self.tracker.output(what, &s.into_bytes());
        }
    }

    /// §598: the fatal error of a DVI file that is too long.
    fn dvi_too_long(&mut self) -> Result<(), Jump> {
        self.dvi.too_long = true;
        self.fatal_error(b"dvi length exceeds \"7FFFFFFF")
    }

    /// §638: output the box `p`: a recorded call (DESIGN 7.17.2) named
    /// by the box, whose writes are the writers' tables (written in its
    /// parts' scopes, `fix_pdfoutput`, `pdf_ship_out`, `ship_box_out`,
    /// and versioned when it ends: `Tex::scoped_call`) and `dead_cycles`,
    /// and whose effects are the page's bytes and the lines its `\write`s
    /// print (each a `write_out` call of its own).
    pub(crate) fn ship_out(&mut self, p: &BoxNode) -> Result<(), Jump> {
        if !T::VALUES {
            return self.ship_out_body(p);
        }
        // (a box shared carries its version, `BoxNode::share`; one built
        // for the page and not shared yet is versioned from its parts)
        let ver = if p.ver == 0 { p.parts_version() } else { p.ver };
        let name = partex_ssa::Version::node(0x7368_6970, &[partex_ssa::Version(ver)]);
        self.scoped_call(crate::ssa::Func::ShipOut, name.0, |t| t.ship_out_body(p))
    }

    fn ship_out_body(&mut self, p: &BoxNode) -> Result<(), Jump> {
        // (the page's sealed lines, opened: what is shipped reads them)
        let open = self.unsealed_deep(p);
        let p = open.as_ref().unwrap_or(p);
        if T::VALUES {
            self.tracker.page();
        }
        let count0 = self.count(0);
        crate::progress::BOARD.page(count0);
        match &mut self.effects {
            Some(e) => e.push(crate::effects::Effect::Shipping(count0)),
            None => self.host.shipping(count0),
        }
        if self.params.flavor == crate::params::Flavor::PdfTex {
            // pdfTeX §791
            self.fix_pdfoutput()?;
            if self.int_par(PDF_OUTPUT_CODE) > 0 {
                self.pdf_ship_out(p, true)?;
                // (a checkpoint is due after each page, `checkpoint_due`:
                // once not counted here, PDF jobs got one per block of
                // lines, ten pages of a book)
                self.shipped += 1;
                return Ok(());
            }
        }
        self.synctex_ship_off();
        let tracing_output = self.int_par(TRACING_OUTPUT_CODE);
        if tracing_output > 0 {
            self.print_nl(b"");
            self.print_ln();
            self.print_str(b"Completed box being shipped out");
        }
        self.print_sep(self.params.max_print_line - 9);
        self.print_char(b'[');
        let mut j = 9;
        while self.count(j) == 0 && j > 0 {
            j -= 1;
        }
        for k in 0..=j {
            self.print_int(self.count(k));
            if k < j {
                self.print_char(b'.');
            }
        }
        self.update_terminal();
        if tracing_output > 0 {
            self.print_char(b']');
            self.begin_diagnostic();
            self.show_box_node(p);
            self.end_diagnostic(true);
        }
        self.out_rtl = false;
        self.ship_box_out(p)?;
        self.shipped += 1;
        if self.etex_ex() {
            self.report_lr_problems();
        }
        if tracing_output <= 0 {
            self.print_char(b']');
        }
        self.set_dead_cycles(0);
        self.update_terminal(); // progress report
        // §639: show statistics if requested (the box itself is the
        // caller's to drop).
        if self.int_par(TRACING_STATS_CODE) > 1 {
            let (var_used, dyn_used) = self.memory_usage();
            self.print_nl(b"Memory usage before: ");
            self.print_int(var_used);
            self.print_char(b'&');
            self.print_int(dyn_used);
            self.print_char(b';');
            self.print_str(b" after: ");
            self.print_int(var_used);
            self.print_char(b'&');
            self.print_int(dyn_used);
            self.print_str(b"; still untouched: ");
            self.print_int(0);
            self.print_ln();
        }
        Ok(())
    }

    /// §640: ship box `p` out: build its page and give it to the backend
    /// (reading and writing the DVI writer's tables).
    fn ship_box_out(&mut self, p: &BoxNode) -> Result<(), Jump> {
        self.writer_scope(DVI_ALL, DVI_ALL, |t| t.ship_box_out_now(p))
    }

    fn ship_box_out_now(&mut self, p: &BoxNode) -> Result<(), Jump> {
        // §641: if the page is too large, `goto done`.
        let v_offset = self.dimen_par(V_OFFSET_CODE);
        let h_offset = self.dimen_par(H_OFFSET_CODE);
        if p.height > MAX_DIMEN
            || p.depth > MAX_DIMEN
            || p.height + p.depth + v_offset > MAX_DIMEN
            || p.width + h_offset > MAX_DIMEN
        {
            self.print_err(b"Huge page cannot be shipped out");
            self.help(&[
                b"The page just created is more than 18 feet tall or",
                b"more than 18 feet wide, so I suspect something went wrong.",
            ]);
            self.error()?;
            if self.int_par(TRACING_OUTPUT_CODE) <= 0 {
                self.begin_diagnostic();
                self.print_nl(b"The following box has been deleted:");
                self.show_box_node(p);
                self.end_diagnostic(true);
            }
            return Ok(());
        }
        self.ensure_dvi_open()?;
        if !self.dvi.started {
            // §617: the preamble.
            self.prepare_mag()?;
            let comment = self.dvi_comment();
            let w = match DviWriter::new(self.params.dvi_buf_size, self.int_par(MAG_CODE), &comment)
            {
                Ok(w) => w,
                Err(TooLong) => return self.dvi_too_long(),
            };
            self.dvi.started = true;
            // (a page sink writes the file itself: not with effects)
            let effects = self.effects.is_some();
            match (self.host.page_sink().filter(|_| !effects), self.dvi.file) {
                (Some(sink), Some(file)) => {
                    self.dvi.bound = u64::try_from(w.length()).unwrap_or(0);
                    if T::VALUES {
                        let mut s = partex_engine::persist::Saver::default();
                        partex_engine::persist::Persist::save(&w, &mut s);
                        self.tracker.output(Output::DviStart, &s.into_bytes());
                    }
                    sink.start(w, file);
                    self.dvi.away = true;
                }
                _ => self.dvi.writer = Some(Val::new(w)),
            }
        }
        let mut counts = [0; 10];
        for (k, c) in (0..).zip(&mut counts) {
            *c = self.count(k);
        }
        let mut page = core::mem::take(&mut self.dvi.page);
        page.clear();
        page.counts = counts;
        page.h_offset = h_offset;
        page.v_offset = v_offset;
        let walked = self.build_list(p, &mut page, false);
        page.truncated = walked.is_err();
        // (by slot, as `page_font` marks them: a font's number is not its
        // slot, `dvi_font_number`)
        self.dvi.page_fonts.fill(false);
        let written = if self.dvi.away {
            self.page_away(page)
        } else {
            let written = self.dvi.writer.as_mut().map_or(Ok(()), |w| w.page(&page));
            if written.is_ok() {
                self.host.page_written(&page);
            }
            self.dvi.page = page;
            self.flush_dvi();
            written
        };
        if written.is_err() {
            return self.dvi_too_long();
        }
        walked
    }

    /// Give `page` to the host's page sink: queued if the file surely
    /// stays below 2^31 bytes, else written at once.
    fn page_away(&mut self, page: Page) -> Result<(), TooLong> {
        // (a margin for what the buffer holds back, §598)
        let limit = 0x7FFF_FFFF - 2 * u64::try_from(self.params.dvi_buf_size).unwrap_or(0);
        if self.host.page_sink().is_none() {
            return Ok(());
        }
        if let Some(b) = page_bound(&page).filter(|b| self.dvi.bound + b < limit) {
            self.dvi.bound += b;
            self.page_effect(Output::DviPage, &page);
            let Some(sink) = self.host.page_sink() else {
                return Ok(());
            };
            self.dvi.page = sink.page(page);
        } else {
            self.page_effect(Output::DviPageNow, &page);
            let Some(sink) = self.host.page_sink() else {
                return Ok(());
            };
            let (page, written) = sink.page_now(page);
            self.dvi.page = page;
            self.dvi.bound = u64::try_from(written?).unwrap_or(0);
        }
        Ok(())
    }

    /// The preamble comment (§617).
    fn dvi_comment(&mut self) -> Vec<u8> {
        if let Some(comment) = &self.params.output_comment {
            return comment.clone();
        }
        let old_setting = self.selector();
        self.set_selector(NEW_STRING);
        self.print_str(b" TeX output ");
        self.print_int(self.int_par(YEAR_CODE));
        self.print_char(b'.');
        self.print_two(self.int_par(MONTH_CODE));
        self.print_char(b'.');
        self.print_two(self.int_par(DAY_CODE));
        self.print_char(b':');
        self.print_two(self.int_par(TIME_CODE) / 60);
        self.print_two(self.int_par(TIME_CODE) % 60);
        self.set_selector(old_setting);
        let start = self.str_start[self.str_ptr];
        let comment = self.str_pool[start..self.pool_ptr].to_vec();
        self.pool_ptr = start; // flush the current string
        comment
    }

    /// Note that the page uses font `f`.
    #[inline]
    fn page_font(&mut self, page: &mut Page, f: i32) {
        let i = ux(f);
        if self.dvi.page_fonts.len() <= i {
            self.dvi.page_fonts.resize(i + 1, false);
        }
        if self.dvi.page_fonts[i] {
            return;
        }
        self.dvi.page_fonts[i] = true;
        let fi = fx(f);
        let text = |s: i32| {
            let s = ux(s);
            self.str_pool[self.str_start[s]..self.str_start[s + 1]].to_vec()
        };
        let font = self.fonts.get(f);
        // (DVI numbers fonts as tex.web does, in the order they were
        // loaded: `FontArrays::number`)
        page.fonts.push(FontDef {
            font: self.dvi_font_number(f),
            check: font.check,
            size: font.size,
            design_size: font.design_size,
            area: text(self.fonts.area[fi]),
            name: text(self.fonts.name[fi]),
        });
    }

    /// §619, §629: append box `this_box` and its contents to the page,
    /// performing its `\write`-like whatsits unless inside leaders. On an
    /// error, the page keeps what was walked and ends at an [`Item::Cut`].
    fn build_list(
        &mut self,
        this_box: &BoxNode,
        page: &mut Page,
        leaders: bool,
    ) -> Result<(), Jump> {
        let h = page.items.len();
        let display = !this_box.vertical && self.etex_ex() && this_box.subtype == lr::DLIST;
        page.items.push(Item::Box {
            vertical: this_box.vertical,
            width: this_box.width,
            height: this_box.height,
            depth: this_box.depth,
            shift: this_box.shift,
            len: 0,
            display,
        });
        let mut glue = SetGlue::default();
        let walked = if !this_box.vertical && self.etex_ex() {
            self.build_hlist_lr(this_box, display, &mut glue, page, leaders)
        } else {
            self.build_items(this_box, &this_box.list, &mut glue, page, leaders)
        };
        // (a nested box that failed has already put the cut last)
        if walked.is_err() && page.items.last() != Some(&Item::Cut) {
            page.items.push(Item::Cut);
        }
        let n = u32::try_from(page.items.len() - h - 1).expect("page size");
        if let Item::Box { len, .. } = &mut page.items[h] {
            *len = n;
        }
        walked
    }

    /// The items of `list`, a part of `this_box`'s list (a discretionary's
    /// replacement text is part of its hlist).
    pub(crate) fn build_items(
        &mut self,
        this_box: &BoxNode,
        list: &[Node],
        glue: &mut SetGlue,
        page: &mut Page,
        leaders: bool,
    ) -> Result<(), Jump> {
        for p in list {
            self.node_item(this_box, p, glue, page, leaders)?;
        }
        Ok(())
    }

    /// §620–§634: the items of node `p` of `this_box`'s list. Returns the
    /// width `p` takes in an hlist.
    pub(crate) fn node_item(
        &mut self,
        this_box: &BoxNode,
        p: &Node,
        glue: &mut SetGlue,
        page: &mut Page,
        leaders: bool,
    ) -> Result<Scaled, Jump> {
        let vertical = this_box.vertical;
        let mut advance = 0;
        match p {
            Node::Glyphs(g) => {
                // §620
                if vertical {
                    return self.confusion(b"vlistout");
                }
                let f = i32::from(g.font.0);
                for &c in g.chars() {
                    advance += self.char_item(f, i32::from(c), page);
                }
            }
            Node::Box(b) => {
                // §623, §632
                advance = b.width;
                if b.list.is_empty() {
                    let d = if vertical {
                        b.height + b.depth
                    } else {
                        b.width
                    };
                    page.items.push(Item::Move(d));
                } else {
                    self.build_list(b, page, leaders)?;
                }
            }
            Node::Rule {
                width,
                height,
                depth,
                ..
            } => {
                advance = *width;
                let r = rule_item(this_box, *height, *depth, *width);
                page.items.push(r);
            }
            Node::Whatsit(w) => match &**w {
                // §1366, §1367
                Whatsit::Special { tokens } => {
                    let start = u32::try_from(page.specials.len()).expect("page size");
                    self.special_text(tokens, &mut page.specials)?;
                    let len = u32::try_from(page.specials.len()).expect("page size") - start;
                    page.items.push(Item::Special { start, len });
                }
                Whatsit::Open { .. } | Whatsit::Write { .. } | Whatsit::Close { .. } => {
                    if !leaders {
                        self.out_what(w)?;
                    }
                }
                Whatsit::LateSpecial { tokens } => {
                    // pdfTeX §1615: `special_out` of a `\special shipout`
                    self.expand_write_text(tokens)?;
                    let start = u32::try_from(page.specials.len()).expect("page size");
                    let expanded = self.take_def();
                    self.special_text(&expanded, &mut page.specials)?;
                    let len = u32::try_from(page.specials.len()).expect("page size") - start;
                    page.items.push(Item::Special { start, len });
                }
                Whatsit::Language { .. } => {}
                Whatsit::Pdf(p) => {
                    // pdfTeX §1620: `out_what` in DVI mode
                    return if matches!(**p, partex_engine::node::PdfWhatsit::SavePos) {
                        self.pdf_error(
                            b"ext4",
                            b"\\pdfsavepos in DVI mode is not implemented in partex yet",
                        )
                    } else {
                        self.pdf_error(b"ext4", b"pdf node ended up in DVI mode")
                    };
                }
            },
            Node::Glue { spec, .. } => {
                // §625, §634: move past glue.
                advance = glue.set(this_box, spec);
                page.items.push(Item::Move(advance));
            }
            Node::Leaders(l) => {
                // §625, §634: output leaders.
                advance = glue.set(this_box, &l.spec);
                self.leaders_items(this_box, l, advance, page)?;
            }
            Node::Kern { width, .. } | Node::MarginKern { width, .. } => {
                advance = *width;
                page.items.push(Item::Move(advance));
            }
            Node::Math { width, .. } if !vertical => {
                advance = *width;
                page.items.push(Item::Move(advance));
            }
            Node::Ligature(l) if !vertical => {
                // §652: the ligature's character.
                advance = self.char_item(i32::from(l.font.0), i32::from(l.ch), page);
            }
            Node::Disc(d) if !vertical => {
                for q in &d.replace {
                    advance += self.node_item(this_box, q, glue, page, leaders)?;
                }
            }
            _ => {}
        }
        Ok(advance)
    }

    /// §626, §635: leaders `l` of size `wd` in `this_box`.
    pub(crate) fn leaders_items(
        &mut self,
        this_box: &BoxNode,
        l: &LeaderNode,
        wd: Scaled,
        page: &mut Page,
    ) -> Result<(), Jump> {
        let vertical = this_box.vertical;
        let item = match &l.leader {
            Node::Rule {
                width,
                height,
                depth,
                ..
            } => {
                // §626, §635: `goto fin_rule`.
                if vertical {
                    rule_item(this_box, wd, 0, *width)
                } else {
                    rule_item(this_box, *height, *depth, wd)
                }
            }
            Node::Box(leader_box) => {
                let leader_size = if vertical {
                    leader_box.height + leader_box.depth
                } else {
                    leader_box.width
                };
                if leader_size > 0 && wd > 0 {
                    let kind = match l.kind {
                        Leaders::Aligned => LeaderKind::Aligned,
                        Leaders::Centered => LeaderKind::Centered,
                        Leaders::Expanded => LeaderKind::Expanded,
                    };
                    let at = page.items.len();
                    page.items.push(Item::Leaders { kind, size: wd });
                    if let Err(j) = self.build_list(leader_box, page, true) {
                        page.items.truncate(at);
                        page.items.push(Item::Cut);
                        return Err(j);
                    }
                    return Ok(());
                }
                Item::Move(wd)
            }
            _ => return self.confusion(b"leaders"),
        };
        page.items.push(item);
        Ok(())
    }
    /// §620: character `c` of font `f` in an hlist.
    /// Returns its width.
    pub(crate) fn char_item(&mut self, f: i32, c: i32, page: &mut Page) -> Scaled {
        self.page_font(page, f);
        // N.B.: `orig_char_info`, not `char_info`
        if let Some(g) = self.fonts.get(f).glyph(c) {
            page.items.push(Item::Char {
                font: self.dvi_font_number(f),
                ch: c,
                width: g.width,
                raise: 0,
            });
            return g.width;
        }
        page.items.push(Item::Missing {
            font: self.dvi_font_number(f),
        });
        if self.mltex_enabled_p {
            self.substitution(f, c, page);
        }
        0
    }

    /// Merged source: `MLTeX`'s "Output a substitution, `goto continue` if
    /// not possible", as IR items after the `Missing` one.
    fn substitution(&mut self, f: i32, c: i32, page: &mut Page) {
        let fi = fx(f);
        // Get substitution information, check it, goto `found` if all is
        // ok, otherwise goto `continue`.
        let sub = self.equiv(CHAR_SUB_CODE_BASE + c);
        let mut ok = None;
        if c >= self.int_par(CHAR_SUB_DEF_MIN_CODE)
            && c <= self.int_par(CHAR_SUB_DEF_MAX_CODE)
            && sub > 0
        {
            let base_c = sub % 256;
            let accent_c = sub / 256;
            let (bc, ec) = (self.fonts.get(f).bc, self.fonts.get(f).ec);
            if ec >= base_c && bc <= base_c && ec >= accent_c && bc <= accent_c {
                // (MLTeX's `char_info`, which goes through `effective_char`)
                let ia = self.effective_char(true, f, accent_c);
                let ib = self.effective_char(true, f, base_c);
                let font = self.fonts.get(f);
                if let (Some(ia_c), Some(ib_c)) = (font.glyph(ia), font.glyph(ib)) {
                    ok = Some((base_c, accent_c, ia_c, ib_c));
                }
            }
            if ok.is_none() {
                self.begin_diagnostic();
                self.print_nl(b"Missing character: Incomplete substitution ");
                self.print(c);
                self.print_str(b" = ");
                self.print(accent_c);
                self.print_str(b" ");
                self.print(base_c);
                self.print_str(b" in font ");
                self.slow_print(self.fonts.name[fi]);
                self.print_char(b'!');
                self.end_diagnostic(false);
                return;
            }
        }
        let Some((base_c, accent_c, ia_c, ib_c)) = ok else {
            self.begin_diagnostic();
            self.print_nl(b"Missing character: There is no ");
            self.print_str(b"substitution for ");
            self.print(c);
            self.print_str(b" in font ");
            self.slow_print(self.fonts.name[fi]);
            self.print_char(b'!');
            self.end_diagnostic(false);
            return;
        };
        // found: print character substitution tracing log.
        if self.int_par(TRACING_LOST_CHARS_CODE) > 99 {
            self.begin_diagnostic();
            self.print_nl(b"Using character substitution: ");
            self.print(c);
            self.print_str(b" = ");
            self.print(accent_c);
            self.print_str(b" ");
            self.print(base_c);
            self.print_str(b" in font ");
            self.slow_print(self.fonts.name[fi]);
            self.print_char(b'.');
            self.end_diagnostic(false);
        }
        // Rebuild character using substitution information.
        let base_x_height = self.font_param(X_HEIGHT_CODE, f);
        let base_slant = f64::from(self.font_param(SLANT_CODE, f)) / 65536.0;
        let accent_slant = base_slant; // slant of accent character font
        let base_width = ib_c.width;
        let base_height = ib_c.height;
        let accent_width = ia_c.width;
        let accent_height = ia_c.height;
        // compute necessary horizontal shift (don't forget slant)
        let delta = zround(
            f64::from(base_width - accent_width) / 2.0 + f64::from(base_height) * base_slant
                - f64::from(base_x_height) * accent_slant,
        );
        // 1. For centering/horizontal shifting insert a kern node; 2. then
        // the accent character, possibly shifted up or down; 3. another
        // kern; 4. the base character.
        let raise = if base_height != base_x_height && accent_height > 0 {
            base_height - base_x_height
        } else {
            0
        };
        page.items.extend([
            Item::Move(delta),
            Item::Char {
                font: self.dvi_font_number(f),
                ch: accent_c,
                width: accent_width,
                raise,
            },
            Item::Move(-accent_width - delta),
            Item::Char {
                font: self.dvi_font_number(f),
                ch: base_c,
                width: base_width,
                raise: 0,
            },
        ]);
    }

    /// §642: finish the DVI file (reading and writing the DVI writer's
    /// tables).
    pub(crate) fn finish_dvi_file(&mut self) -> Result<(), Jump> {
        self.writer_scope(DVI_ALL, DVI_ALL, Self::finish_dvi_file_now)
    }

    fn finish_dvi_file_now(&mut self) -> Result<(), Jump> {
        if self.dvi.too_long {
            return Ok(());
        }
        if !self.dvi.started {
            self.print_nl(b"No pages of output.");
            return Ok(());
        }
        self.prepare_mag()?;
        let mag = self.int_par(MAG_CODE);
        let summary = if self.dvi.away {
            if T::VALUES {
                self.tracker.output(Output::DviFinish, &mag.to_le_bytes());
            }
            self.host.page_sink().map(|s| s.finish(mag))
        } else {
            self.dvi.writer.as_mut().map(|w| w.finish(mag))
        };
        self.flush_dvi();
        let Some(Ok(summary)) = summary else {
            return self.dvi_too_long();
        };
        self.print_nl(b"Output written on ");
        self.print_file_name(0, self.output_file_name(), 0);
        self.print_str(b" (");
        self.print_int(summary.pages);
        if summary.pages == 1 {
            self.print_str(b" page");
        } else {
            self.print_str(b" pages");
        }
        self.print_str(b", ");
        self.print_int(summary.bytes);
        self.print_str(b" bytes).");
        if let Some(id) = self.dvi.file.take() {
            self.out_close(id);
        }
        Ok(())
    }

    /// §1368: append the text of a `\special` to `out`.
    fn special_text(&mut self, tokens: &Tokens, out: &mut Vec<u8>) -> Result<(), Jump> {
        let old_setting = self.selector();
        self.set_selector(NEW_STRING);
        // encTeX's `\specialout`/`\mubyteout` handling is inert without
        // encTeX (the stored stream is `mubyte_zero`, so `spec_out = 0`).
        self.active_noconvert = true;
        let l = i32::try_from(self.pool_size() - self.pool_ptr).unwrap_or(i32::MAX);
        self.show_token_slice(tokens, l); // `write_tokens(p)`
        self.set_selector(old_setting);
        self.str_room(1)?;
        let start = self.str_start[self.str_ptr];
        out.extend_from_slice(&self.str_pool[start..self.pool_ptr]);
        self.special_printing = false;
        self.cs_converting = false;
        self.active_noconvert = false;
        self.pool_ptr = start; // erase the string
        Ok(())
    }

    /// §1371: expand macros in `tokens` (a `\write` or `\special
    /// shipout` text) and make `link(def_ref)` point to the result.
    pub(crate) fn expand_write_text(&mut self, tokens: &Tokens) -> Result<(), Jump> {
        let q = self.tok_from(&[
            RIGHT_BRACE_TOKEN + i32::from(b'}'),
            crate::expand::END_WRITE_TOKEN,
        ]);
        self.ins_list(q)?;
        self.begin_token_list(tokens.clone(), WRITE_TEXT)?;
        let q = self.tok_from(&[LEFT_BRACE_TOKEN + i32::from(b'{')]);
        self.ins_list(q)?;
        // now we're ready to scan `{<token list>} \endwrite`
        let old_mode = self.mode();
        self.set_mode(0); // disable \prevdepth, \spacefactor, \lastskip, \prevgraf
        self.cur_cs = self.write_loc;
        self.scan_toks(false, true)?; // expand macros, etc.
        self.get_token()?;
        if self.cur_tok != crate::expand::END_WRITE_TOKEN {
            // §1372: recover from an unbalanced write command.
            self.print_err(b"Unbalanced write command");
            self.help(&[
                b"On this page there's a \\write with fewer real {'s than }'s.",
                b"I can't handle that very well; good luck.",
            ]);
            self.error()?;
            loop {
                self.get_token()?;
                if self.cur_tok == crate::expand::END_WRITE_TOKEN {
                    break;
                }
            }
        }
        self.set_mode(old_mode);
        self.end_token_list()?; // conserve stack space
        Ok(())
    }

    /// §1370: write the token list of a `\write` node to stream `j`: a
    /// recorded call (DESIGN 7.17.2) named by the stream and the tokens
    /// (by the version the list carries), whose effects are the line's
    /// bytes and whose store is the line (7.17.5).
    fn write_out(&mut self, j: i32, tokens: &Tokens) -> Result<(), Jump> {
        if !T::VALUES {
            return self.write_out_body(j, tokens);
        }
        // (a list made before versions were on, as a format's, is
        // versioned by its tokens here)
        let tv = match tokens.version() {
            0 => tokens.version_by_tokens(),
            v => v,
        };
        let name = partex_ssa::Version::node(
            0x7772_6974,
            &[partex_ssa::Version::of(&j), partex_ssa::Version(tv)],
        );
        self.scoped_call(crate::ssa::Func::WriteOut, name.0, |t| {
            t.write_out_body(j, tokens)
        })
    }

    fn write_out_body(&mut self, j: i32, tokens: &Tokens) -> Result<(), Jump> {
        self.expand_write_text(tokens)?;
        let old_setting = self.selector();
        if let Ok(n @ 0..16) = u8::try_from(j) {
            // (the file stream `j` stores to)
            self.out_read(n);
        }
        if j == 18 {
            self.set_selector(NEW_STRING);
        } else if (0..16).contains(&j) && self.write_open(ux(j)) {
            self.set_selector(j);
        } else {
            // write to the terminal if file isn't open
            if j == 17 && self.selector() == TERM_AND_LOG {
                self.set_selector(LOG_ONLY);
            }
            self.print_nl(b"");
        }
        self.active_noconvert = true;
        // (a note for a host that asks: what a write to the terminal says)
        let note = j != 18
            && matches!(self.selector(), TERM_AND_LOG | crate::print::TERM_ONLY)
            && self.notes_wanted();
        let p = core::mem::take(&mut self.def_ref);
        if note {
            let text = self.diag_print(|t| t.token_show(&p));
            self.diag_note(crate::diag::Severity::Note, "write", text, None);
        }
        self.token_show(&p);
        self.print_ln();
        self.cs_converting = false;
        self.active_noconvert = false;
        if j == 18 {
            let sel = if self.int_par(TRACING_ONLINE_CODE) <= 0 {
                LOG_ONLY // show what we're doing in the log file
            } else {
                TERM_AND_LOG // show what we're doing
            };
            self.set_selector(sel);
            // If the log file isn't open yet, we can only send output to
            // the terminal.
            if !self.log_opened() {
                self.set_selector(crate::print::TERM_ONLY);
            }
            self.print_nl(b"runsystem(");
            let start = self.str_start[self.str_ptr];
            for d in 0..self.cur_length() {
                // `print` gives up if passed `str_ptr`, so do it by hand
                self.print(i32::from(self.str_pool[start + d])); // N.B.: not `print_char`
            }
            self.print_str(b")...");
            if self.params.shell_escape {
                let cmd: alloc::vec::Vec<u8> = self.str_pool[start..self.pool_ptr]
                    .iter()
                    .map(|&c| self.xchr[usize::from(c)])
                    .collect();
                // (minimal checking: NUL not allowed in the argument
                // string of `system`, but as its last character it only
                // ends the string)
                if cmd.split_last().is_some_and(|(_, init)| init.contains(&0)) {
                    self.print_str(b"clobbered");
                } else {
                    let cmd = cmd.strip_suffix(&[0]).unwrap_or(&cmd);
                    // We have the command. See if we're allowed to execute
                    // it, and report in the log. We don't check the actual
                    // exit status of the command.
                    let decision = crate::shell::runsystem(
                        cmd,
                        self.params.restricted_shell,
                        &self.params.shell_escape_commands,
                    );
                    if let Some(run) = decision.command() {
                        self.run_command(run);
                    }
                    self.print_str(match decision {
                        crate::shell::Decision::QuotationError => {
                            b"quotation error in system command"
                        }
                        crate::shell::Decision::Restricted => b"disabled (restricted)",
                        crate::shell::Decision::Any(_) => b"executed",
                        crate::shell::Decision::Allowed(_) => b"executed safely (allowed)",
                    });
                }
            } else {
                self.print_str(b"disabled"); // `shellenabledp` false
            }
            self.print_char(b'.');
            self.print_nl(b"");
            self.print_ln();
            self.pool_ptr = self.str_start[self.str_ptr]; // erase the string
        }
        self.set_selector(old_setting);
        Ok(())
    }

    /// Run `\write18`'s command `cmd` (allowed and quoted) through the
    /// host. In an SSA build (DESIGN 3.7, "Commands") the command reads
    /// the job's files closed before it, loads of the build's stores that
    /// it is handed, and each file it made or changed is a store of the
    /// step.
    fn run_command(&mut self, cmd: &[u8]) {
        // (a run that read a definition it was not placed at is dropped
        // and made again: its command would see the wrong files, and
        // leave what it wrote)
        if T::VALUES && self.tracker.run_doomed() {
            return;
        }
        let inputs = if T::VALUES {
            // (a file still open for writing is not an input: what of it
            // a command finds on disk is whatever the writer flushed)
            let open: Vec<alloc::sync::Arc<[u8]>> =
                (0..16).filter_map(|n| self.streams.out_name(n)).collect();
            let inputs: Vec<_> = self
                .tracker
                .stores_reaching()
                .into_iter()
                .filter(|(name, _)| !open.iter().any(|o| o[..] == name[..]))
                .collect();
            for (name, contents) in &inputs {
                self.tracker.load(name, FileKind::Tex, Some(contents));
            }
            inputs
        } else {
            Vec::new()
        };
        let Some(ran) = self.host.system(cmd, &inputs) else {
            return;
        };
        if !ran.stdout.is_empty() {
            if T::VALUES {
                self.tracker.output(crate::track::Output::Term, &ran.stdout);
            }
            self.out_term(&ran.stdout);
        }
        if T::VALUES {
            for (name, contents) in &ran.wrote {
                self.tracker.command_wrote(name, contents);
            }
            for name in &ran.removed {
                self.tracker.command_removed(name);
            }
        }
    }

    /// §1373, §1374: perform an `\openout`, `\write` or `\closeout`
    /// during `ship_out` (outside leaders) or `\immediate`.
    pub(crate) fn out_what(&mut self, w: &Whatsit) -> Result<(), Jump> {
        let j = match w {
            Whatsit::Write { stream, tokens } => return self.write_out(*stream, tokens),
            Whatsit::Open { stream, .. } | Whatsit::Close { stream } => *stream,
            // (`\special`s become page items; `\immediate` only performs
            // the others)
            Whatsit::Special { .. }
            | Whatsit::LateSpecial { .. }
            | Whatsit::Language { .. }
            | Whatsit::Pdf(_) => return Ok(()),
        };
        // §1374: do some work that has been queued up for \write.
        let ju = ux(j);
        if let Ok(n @ 0..16) = u8::try_from(j) {
            // (an open file is closed first)
            self.out_read(n);
            if !T::VALUES {
                self.tracker.write(crate::track::Cell::Out(j));
            }
        }
        if ju < 16 && self.write_open(ju) {
            if let Some(id) = self.write_file[ju].id.take() {
                let buf = core::mem::take(&mut self.write_file[ju].buf);
                self.write_file_bytes(id, &buf);
                self.write_file_close(id);
            }
            self.set_write_open(ju, false);
            self.set_out_name(ju, None);
            self.out_wrote(u8::try_from(ju).unwrap_or(0));
        }
        let Whatsit::Open {
            name, area, ext, ..
        } = w
        else {
            return Ok(()); // `close_node`: already closed
        };
        if j >= 16 {
            return Ok(());
        }
        let ext: &[u8] = if ext.is_empty() { b".tex" } else { ext };
        self.pack_file_name_bytes(name, area, ext);
        loop {
            let file = self.name_of_file.clone();
            if let Some((id, _)) = self.open_out(&file, FileKind::Other) {
                self.write_file[ju].id = Some(id);
                // (the file stream `j` stores to: a store made, 7.17.5)
                self.set_out_name(ju, Some(&file));
                self.out_wrote(u8::try_from(ju).unwrap_or(0));
                break;
            }
            // (`prompt_file_name` shows and replaces `cur_name` etc.)
            self.cur_name = self.make_tex_string(name)?;
            self.cur_area = self.make_tex_string(area)?;
            self.cur_ext = self.make_tex_string(ext)?;
            self.prompt_file_name(b"output file name", b".tex")?;
        }
        self.set_write_open(ju, true);
        // If on first line of input, log file is not ready yet, so don't
        // log.
        if self.log_opened() && self.params.log_openout {
            let old_setting = self.selector();
            let sel = if self.int_par(TRACING_ONLINE_CODE) <= 0 {
                LOG_ONLY
            } else {
                TERM_AND_LOG
            };
            self.set_selector(sel);
            self.print_nl(b"\\openout");
            self.print_int(j);
            self.print_str(b" = `");
            self.print_file_name_bytes(name, area, ext);
            self.print_str(b"'.");
            self.print_nl(b"");
            self.print_ln();
            self.set_selector(old_setting);
        }
        Ok(())
    }
}

/// §625, §634: the glue of a box set so far.
#[derive(Default)]
pub(crate) struct SetGlue {
    cur_g: Scaled,
    cur_glue: f64,
}

impl SetGlue {
    /// The size of glue `g` in `this_box`.
    pub(crate) fn set(&mut self, this_box: &BoxNode, g: &GlueSpec) -> Scaled {
        let mut wd = g.width - self.cur_g;
        match this_box.glue_sign {
            GlueSign::Normal => {}
            GlueSign::Stretching => {
                if g.stretch_order == this_box.glue_order {
                    self.cur_glue += f64::from(g.stretch);
                    self.cur_g = zround(vet_glue(this_box.glue_set * self.cur_glue));
                }
            }
            GlueSign::Shrinking => {
                if g.shrink_order == this_box.glue_order {
                    self.cur_glue -= f64::from(g.shrink);
                    self.cur_g = zround(vet_glue(this_box.glue_set * self.cur_glue));
                }
            }
        }
        wd += self.cur_g;
        wd
    }
}

/// §624, §633: a rule of `this_box` with its running dimensions resolved.
fn rule_item(this_box: &BoxNode, mut height: Scaled, mut depth: Scaled, mut width: Scaled) -> Item {
    if this_box.vertical {
        if width == RUNNING {
            width = this_box.width;
        }
    } else {
        if height == RUNNING {
            height = this_box.height;
        }
        if depth == RUNNING {
            depth = this_box.depth;
        }
    }
    Item::Rule {
        height,
        depth,
        width,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{engine, term_output};
    use alloc::vec;
    use partex_engine::node::Glyphs;

    fn hbox(width: Scaled, height: Scaled, list: Vec<Node>) -> BoxNode {
        BoxNode {
            width,
            height,
            list,
            ..BoxNode::default()
        }
    }

    fn glue(w: Scaled) -> Node {
        Node::Glue {
            spec: GlueSpec {
                width: w,
                ..GlueSpec::default()
            },
            subtype: 0,
            sync: partex_engine::origin::Side(0),
        }
    }

    /// `testdata/ship.dvi` is `tex -ini -output-comment=partex` run on
    /// `testdata/ship.tex`; the boxes are built here as its `\showbox`
    /// shows them (hpack isn't needed to test the DVI writer).
    #[test]
    fn ship_out_matches_tex() {
        let mut t = engine();
        t.init_prim().unwrap();
        t.init_null_font();
        t.params.output_comment = Some(b"partex".to_vec());
        t.host.files.insert(
            b"cmr10.tfm".to_vec(),
            include_bytes!("../testdata/cmr10.tfm").to_vec(),
        );
        let nom = t.make_tex_string(b"cmr10").unwrap();
        let aire = t.pool_str(b"");
        let f = t.read_font_info(0, nom, aire, -1000).unwrap();
        t.job_name = t.make_tex_string(b"t").unwrap();
        let pt = 65536;
        let fid = crate::fonts::font_id(f);
        let ch = |c: u8| Node::Glyphs(Glyphs::one(fid, c));
        let metric = |t: &Tex<_, _>, c: u8| {
            let g = t.fonts.get(f).glyph(i32::from(c)).unwrap();
            (g.width, g.height)
        };
        let (wa, ha) = metric(&t, b'A');
        let (wv, _) = metric(&t, b'V');
        let (ww, _) = metric(&t, b'W');
        // \vbox{\hrule height 1pt width 3pt\kern2pt\hbox{A}\vskip 3pt\hrule depth 1pt}
        let inner = hbox(wa, ha, vec![ch(b'A')]);
        let vh = pt + 2 * pt + ha + 3 * pt + 26214 + pt;
        let v = BoxNode {
            vertical: true,
            width: wa,
            height: vh,
            shift: -2 * pt,
            list: vec![
                Node::Rule {
                    width: 3 * pt,
                    height: pt,
                    depth: 0,
                    sync: partex_engine::origin::Side(0),
                },
                Node::Kern {
                    width: 2 * pt,
                    subtype: 0,
                    sync: partex_engine::origin::Side(0),
                },
                Node::Box(inner.share()),
                glue(3 * pt),
                Node::Rule {
                    width: RUNNING,
                    height: 26214, // default rule height 0.4pt
                    depth: pt,
                    sync: partex_engine::origin::Side(0),
                },
            ],
            ..BoxNode::default()
        };
        let kern = |w| Node::Kern {
            width: w,
            subtype: 0,
            sync: partex_engine::origin::Side(0),
        };
        let b0 = hbox(
            wa - 72819 + wv + 10 * pt + 5 * pt + wa + 10 * pt + 2 * ww,
            vh + 2 * pt,
            vec![
                ch(b'A'),
                kern(-72819),
                ch(b'V'),
                kern(3 * pt),
                Node::Rule {
                    width: 2 * pt,
                    height: RUNNING,
                    depth: RUNNING,
                    sync: partex_engine::origin::Side(0),
                },
                glue(5 * pt),
                Node::Box(v.share()),
                glue(5 * pt),
                ch(b'W'),
                glue(5 * pt),
                ch(b'W'),
            ],
        );
        // \hbox{\kern 400pt A}
        let b1 = hbox(400 * pt + wa, ha, vec![kern(400 * pt), ch(b'A')]);
        let out = term_output(&mut t, |t| {
            t.ship_out(&b0).unwrap();
            t.ship_out(&b1).unwrap();
            t.finish_dvi_file().unwrap();
        });
        assert_eq!(
            core::str::from_utf8(&out).unwrap(),
            "[0] [0]\nOutput written on t.dvi (2 pages, 280 bytes)."
        );
        let dvi = &t.host.written[&1];
        assert_eq!(dvi.as_slice(), include_bytes!("../testdata/ship.dvi"));
        // The IR of the last page, as text.
        assert_eq!(
            t.dvi.page.dump(),
            "page [0, 0, 0, 0, 0, 0, 0, 0, 0, 0] offset=(0,0)\n\
             font 1 cmr10 at 655360 design 655360 check [4b, f1, 60, 79]\n\
             hbox wd=26705921 ht=447828 dp=0 shift=0\n  \
               move 26214400\n  \
               char f1 65 wd=491521\n"
        );
    }
}
