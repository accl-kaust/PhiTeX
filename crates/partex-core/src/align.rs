//! Part 37: Alignment (§768–§812). Finishing an alignment is the
//! engine's ([`partex_engine::align::fin_align`]).

use alloc::boxed::Box;
use alloc::vec::Vec;

use partex_engine::align::{self, Column, NULL_FLAG};
use partex_engine::node::{GlueSpec, Node, Unset};
use partex_engine::nodelist::NodeList;
use partex_engine::pack::{Spec, order};
use partex_engine::persist::{Loader, Persist, Saver};
use partex_ssa::pvec::Poly;
use partex_ssa::{PVec, Value, Version};

use crate::arith::Scaled;
use crate::host::Host;
use crate::mem::NULL;
use crate::nest::IGNORE_DEPTH;
use crate::nodes::param_glue;
use crate::pack::spec;
use crate::scan::MAX_DIMEN;
use crate::tex::{Jump, Tex};
use crate::track::{Row, Tracker, align as align_row};
use crate::web::*;

/// §769: an alignrecord: a column's templates, what began its current
/// entry, and its widths; a value carrying its version, made when the
/// record is made or changed ([`AlignRecord::remade`]).
#[derive(Clone, Debug)]
pub(crate) struct AlignRecord {
    /// `u_part`, `v_part`: token lists.
    pub(crate) u: crate::tok::Tokens,
    pub(crate) v: crate::tok::Tokens,
    /// `extra_info`: the command that began the entry, then the one that
    /// ended it.
    pub(crate) extra_info: i32,
    pub(crate) column: Column,
    ver: Version,
}

impl AlignRecord {
    /// A column of templates `u` and `v` that no entry has used yet.
    fn new(u: crate::tok::Tokens, v: crate::tok::Tokens) -> AlignRecord {
        AlignRecord {
            u,
            v,
            extra_info: 0,
            column: Column {
                width: NULL_FLAG,
                spans: Vec::new(),
            },
            ver: Version::ABSENT,
        }
        .remade()
    }

    /// The record with its version made from its parts (the templates by
    /// the versions they carry).
    fn remade(mut self) -> AlignRecord {
        self.ver = Version::node(
            0x616c_7263,
            &[
                Version(self.u.version()),
                Version(self.v.version()),
                Version::of(&(self.extra_info, &self.column)),
            ],
        );
        self
    }
}

impl Value for AlignRecord {
    fn version(&self) -> Version {
        self.ver
    }
}

impl Persist for AlignRecord {
    fn save(&self, s: &mut Saver) {
        self.u.save(s);
        self.v.save(s);
        self.extra_info.save(s);
        self.column.save(s);
    }
    fn load(l: &mut Loader) -> Option<Self> {
        let u = Persist::load(l)?;
        let v = Persist::load(l)?;
        let extra_info = Persist::load(l)?;
        let column = Persist::load(l)?;
        Some(
            AlignRecord {
                u,
                v,
                extra_info,
                column,
                ver: Version::ABSENT,
            }
            .remade(),
        )
    }
}

/// §778: a preamble's tabskip glue, before each column and after the
/// last: appended to while the preamble grows (§778, §793), versioned by
/// a running polynomial of its entries.
#[derive(Clone, Debug)]
pub(crate) struct Tabskips {
    glue: Vec<GlueSpec>,
    poly: Poly,
}

impl Default for Tabskips {
    fn default() -> Self {
        Tabskips {
            glue: Vec::new(),
            poly: Poly::EMPTY,
        }
    }
}

impl Tabskips {
    fn push(&mut self, g: GlueSpec) {
        self.poly = self.poly.then(Poly::unit(Version::of(&g)));
        self.glue.push(g);
    }

    pub(crate) fn glue(&self) -> &[GlueSpec] {
        &self.glue
    }

    fn version(&self) -> Version {
        self.poly.version(self.glue.len())
    }
}

impl Persist for Tabskips {
    fn save(&self, s: &mut Saver) {
        self.glue.save(s);
    }
    fn load(l: &mut Loader) -> Option<Self> {
        let glue: Vec<GlueSpec> = Persist::load(l)?;
        let mut t = Tabskips::default();
        for g in glue {
            t.push(g);
        }
        Some(t)
    }
}

/// §770: the state of one alignment (the alignment stack holds those of
/// the enclosing ones). Its fields are the `align` row's values (DESIGN
/// 7.17.12, `track::align`), each read and written through the
/// accessors below.
#[derive(Clone, Debug, Default)]
pub(crate) struct AlignLevel {
    /// The preamble: the columns (a persistent sequence, versioned by its
    /// records), and the tabskip glue before each column and after the
    /// last (one more).
    pub(crate) columns: PVec<AlignRecord>,
    pub(crate) tabskips: Tabskips,
    /// `cur_align`: the current column.
    pub(crate) cur_align: Option<usize>,
    /// `cur_span`: the column where the current span began.
    pub(crate) cur_span: usize,
    /// `cur_loop`: the tabskip glue before the next column to copy in a
    /// periodic preamble.
    pub(crate) cur_loop: Option<usize>,
    /// Material migrating out of the current row (`cur_head` list).
    pub(crate) adjust: NodeList,
    /// The enclosing alignment's `align_state` (saved by
    /// `push_alignment`).
    pub(crate) align_state: i32,
}

impl Persist for AlignLevel {
    fn save(&self, s: &mut Saver) {
        self.columns.to_vec().save(s);
        self.tabskips.save(s);
        self.cur_align.save(s);
        self.cur_span.save(s);
        self.cur_loop.save(s);
        self.adjust.save(s);
        self.align_state.save(s);
    }
    fn load(l: &mut Loader) -> Option<Self> {
        let columns: Vec<AlignRecord> = Persist::load(l)?;
        Some(AlignLevel {
            columns: PVec::from_vec(columns),
            tabskips: Persist::load(l)?,
            cur_align: Persist::load(l)?,
            cur_span: Persist::load(l)?,
            cur_loop: Persist::load(l)?,
            adjust: Persist::load(l)?,
            align_state: Persist::load(l)?,
        })
    }
}

impl AlignLevel {
    /// The level's version, from its fields' (it enters the stack whole).
    fn version(&self) -> Version {
        Version::node(
            0x616c_6c76,
            &[
                self.columns.version(),
                self.tabskips.version(),
                Version::of(&(
                    self.cur_align,
                    self.cur_span,
                    self.cur_loop,
                    self.align_state,
                )),
                self.adjust.version(),
            ],
        )
    }
}

/// The alignments (§770): the current one and the stack of those it is
/// inside, each on the stack with the stack's version up to it (a push
/// and a pop are O(1) on the version).
#[derive(Clone, Debug, Default)]
pub(crate) struct AlignState {
    pub(crate) cur: AlignLevel,
    pub(crate) stack: Vec<(AlignLevel, Version)>,
}

impl Persist for AlignState {
    fn save(&self, s: &mut Saver) {
        self.cur.save(s);
        let levels: Vec<&AlignLevel> = self.stack.iter().map(|(l, _)| l).collect();
        levels.len().save(s);
        for l in levels {
            l.save(s);
        }
    }
    fn load(l: &mut Loader) -> Option<Self> {
        let cur = AlignLevel::load(l)?;
        let n = usize::load(l)?;
        let mut a = AlignState {
            cur,
            stack: Vec::new(),
        };
        for _ in 0..n {
            a.push(AlignLevel::load(l)?);
        }
        Some(a)
    }
}

impl AlignState {
    fn push(&mut self, level: AlignLevel) {
        let below = self.stack_version();
        let v = Version::node(0x616c_7374, &[below, level.version()]);
        self.stack.push((level, v));
    }

    fn stack_version(&self) -> Version {
        self.stack.last().map_or(Version::ABSENT, |(_, v)| *v)
    }

    /// Field `f`'s version (`track::align`), from the values' own.
    fn field_version(&self, f: u8) -> Version {
        use crate::track::align::*;
        let c = &self.cur;
        match f {
            COLUMN => Version::of(&c.cur_align),
            SPAN => Version::of(&c.cur_span),
            LOOP => Version::of(&c.cur_loop),
            ADJUST => c.adjust.version(),
            COLUMNS => c.columns.version(),
            TABSKIPS => c.tabskips.version(),
            _ => self.stack_version(),
        }
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    // The `align` row's fields as values (DESIGN 7.17.12, `track::align`):
    // each read tells the tracker the field's version, each change is a
    // read of it and a write.
    #[inline]
    fn align_read(&self, f: u8) {
        if T::VALUES {
            self.tracker
                .value_read(Row::Align(f), || self.align.field_version(f).0);
        }
    }

    #[inline]
    fn align_wrote(&self, f: u8) {
        if T::VALUES {
            self.tracker.value_wrote(Row::Align(f));
        }
    }

    #[inline]
    fn align_changed(&self, f: u8) {
        self.align_read(f);
        self.align_wrote(f);
    }

    /// The version of the `align` row's field `f` now.
    pub(crate) fn align_version(&self, f: u8) -> u128 {
        self.align.field_version(f).0
    }

    /// `cur_align` (§770).
    fn cur_align(&self) -> Option<usize> {
        self.align_read(align_row::COLUMN);
        self.align.cur.cur_align
    }
    fn set_cur_align(&mut self, v: Option<usize>) {
        self.align.cur.cur_align = v;
        self.align_wrote(align_row::COLUMN);
    }
    /// `cur_span` (§770).
    fn cur_span(&self) -> usize {
        self.align_read(align_row::SPAN);
        self.align.cur.cur_span
    }
    fn set_cur_span(&mut self, v: usize) {
        self.align.cur.cur_span = v;
        self.align_wrote(align_row::SPAN);
    }
    /// `cur_loop` (§770).
    fn cur_loop(&self) -> Option<usize> {
        self.align_read(align_row::LOOP);
        self.align.cur.cur_loop
    }
    fn set_cur_loop(&mut self, v: Option<usize>) {
        self.align.cur.cur_loop = v;
        self.align_wrote(align_row::LOOP);
    }
    /// The preamble's number of columns.
    fn column_count(&self) -> usize {
        self.align_read(align_row::COLUMNS);
        self.align.cur.columns.len()
    }
    /// Column `j` of the preamble.
    fn column(&self, j: usize) -> &AlignRecord {
        self.align_read(align_row::COLUMNS);
        self.align.cur.columns.get(j).expect("alignrecord")
    }
    /// Column `j` of the preamble, changed by `f` (and versioned again).
    fn edit_column<R>(&mut self, j: usize, f: impl FnOnce(&mut AlignRecord) -> R) -> R {
        self.align_changed(align_row::COLUMNS);
        let mut r = self.align.cur.columns.get(j).expect("alignrecord").clone();
        let x = f(&mut r);
        self.align.cur.columns.set(j, r.remade());
        x
    }
    /// A column appended to the preamble.
    fn push_column(&mut self, r: AlignRecord) {
        self.align_changed(align_row::COLUMNS);
        self.align.cur.columns.push(r);
    }
    /// The current column (`cur_align`, §770) changed by `f`; `None` if
    /// there is none.
    pub(crate) fn edit_cur_column<R>(
        &mut self,
        f: impl FnOnce(&mut AlignRecord) -> R,
    ) -> Option<R> {
        let a = self.cur_align()?;
        (a < self.column_count()).then(|| self.edit_column(a, f))
    }
    /// The tabskip glue before column `j` (after the last for `j` the
    /// number of columns).
    fn tabskip(&self, j: usize) -> GlueSpec {
        self.align_read(align_row::TABSKIPS);
        self.align.cur.tabskips.glue()[j]
    }
    fn tabskip_count(&self) -> usize {
        self.align_read(align_row::TABSKIPS);
        self.align.cur.tabskips.glue().len()
    }
    fn push_tabskip(&mut self, g: GlueSpec) {
        self.align_changed(align_row::TABSKIPS);
        self.align.cur.tabskips.push(g);
    }
    /// The material migrating out of the current row, to change.
    fn row_adjust_mut(&mut self) -> &mut NodeList {
        self.align_changed(align_row::ADJUST);
        &mut self.align.cur.adjust
    }

    /// The current alignment's state, taken (every field read and
    /// written: the level is left empty).
    fn take_align_level(&mut self) -> AlignLevel {
        for f in align_row::LEVEL {
            self.align_changed(f);
        }
        core::mem::take(&mut self.align.cur)
    }

    /// The current alignment's state set to `level` (every field written).
    fn set_align_level(&mut self, level: AlignLevel) {
        self.align.cur = level;
        for f in align_row::LEVEL {
            self.align_wrote(f);
        }
    }

    /// §772
    fn push_alignment(&mut self) {
        let mut old = self.take_align_level();
        old.align_state = self.align_state();
        self.align_changed(align_row::STACK);
        self.align.push(old);
    }

    /// §772
    fn pop_alignment(&mut self) {
        self.align_changed(align_row::STACK);
        let (old, _) = self.align.stack.pop().unwrap_or_default();
        self.set_align_state(old.align_state);
        self.set_align_level(old);
    }

    /// §774
    pub(crate) fn init_align(&mut self) -> Result<(), Jump> {
        let save_cs_ptr = self.cur_cs; // \halign or \valign, usually
        self.push_alignment();
        self.set_align_state(-1_000_000); // enter a new alignment level
        // §776: check for improper alignment in displayed math.
        if self.mode() == MMODE && (!self.list_is_empty() || self.incompleat().is_some()) {
            self.print_err(b"Improper ");
            self.print_esc(b"halign");
            self.print_str(b" inside $$'s");
            self.help(&[
                b"Displays can use special alignments (like \\eqalignno)",
                b"only if nothing but the alignment itself is between $$'s.",
                b"So I've deleted the formulas that preceded this alignment.",
            ]);
            self.error()?;
            self.flush_math();
        }
        self.push_nest()?; // enter a new semantic level
        // §775: change current mode to `-vmode` for \halign, `-hmode` for
        // \valign.
        if self.mode() == MMODE {
            self.set_mode(-VMODE);
            let pd = self.level_prev_depth(self.nest_ptr() - 2);
            self.set_prev_depth(pd);
        } else if self.mode() > 0 {
            self.set_mode(-self.mode());
        }
        self.scan_spec(ALIGN_GROUP, false)?;
        // §777: scan the preamble and record it in the `preamble` list.
        self.set_align_level(AlignLevel::default());
        self.scanner_status = ALIGNING;
        self.warning_index = save_cs_ptr;
        self.set_align_state(-1_000_000); // at this point, `cur_cmd=left_brace`
        loop {
            // §778: append the current tabskip glue to the preamble list.
            let g = self.glue_par(TAB_SKIP_CODE);
            self.push_tabskip(g);
            if self.cur_cmd == CAR_RET {
                break; // \cr ends the preamble
            }
            // §779: scan preamble text until `cur_cmd` is `tab_mark` or
            // `car_ret`, looking for changes in the tabskip glue; append
            // an alignrecord to the preamble list.
            // §783: scan the template <u_j> into a new token list.
            self.preamble_list.clear();
            self.preamble_active = true;
            loop {
                self.get_preamble_token()?;
                if self.cur_cmd == MAC_PARAM {
                    break;
                }
                if self.cur_cmd <= CAR_RET
                    && self.cur_cmd >= TAB_MARK
                    && self.align_state() == -1_000_000
                {
                    if self.preamble_list.is_empty()
                        && self.cur_loop().is_none()
                        && self.cur_cmd == TAB_MARK
                    {
                        // the tabskip glue just appended
                        let l = self.tabskip_count() - 1;
                        self.set_cur_loop(Some(l));
                    } else {
                        self.print_err(b"Missing # inserted in alignment preamble");
                        self.help(&[
                            b"There should be exactly one # between &'s, when an",
                            b"\\halign or \\valign is being set up. In this case you had",
                            b"none, so I've put one in; maybe that will work.",
                        ]);
                        self.back_error()?;
                        break;
                    }
                } else if self.cur_cmd != SPACER || !self.preamble_list.is_empty() {
                    self.preamble_list.push(self.cur_tok);
                }
            }
            let u = core::mem::take(&mut self.preamble_list);
            let u = self.make_list(u);
            // §784: scan the template <v_j> into a new token list.
            // (the alignrecord is in the preamble while <v_j> is scanned)
            let v = self.null_list();
            self.push_column(AlignRecord::new(u, v));
            loop {
                self.get_preamble_token()?;
                if self.cur_cmd <= CAR_RET
                    && self.cur_cmd >= TAB_MARK
                    && self.align_state() == -1_000_000
                {
                    break;
                }
                if self.cur_cmd == MAC_PARAM {
                    self.print_err(b"Only one # is allowed per tab");
                    self.help(&[
                        b"There should be exactly one # between &'s, when an",
                        b"\\halign or \\valign is being set up. In this case you had",
                        b"more than one, so I'm ignoring all but the first.",
                    ]);
                    self.error()?;
                    continue;
                }
                self.preamble_list.push(self.cur_tok);
            }
            self.preamble_list.push(END_TEMPLATE_TOKEN); // put \endtemplate at the end
            let v = core::mem::take(&mut self.preamble_list);
            let v = self.make_list(v);
            let last = self.column_count() - 1;
            self.edit_column(last, |r| r.v = v);
        }
        self.preamble_list.clear();
        self.preamble_active = false;
        self.scanner_status = NORMAL;
        self.new_save_level(ALIGN_GROUP)?;
        self.begin_toks_at(EVERY_CR_LOC, EVERY_CR_TEXT)?;
        self.align_peek() // look for \noalign or \omit
    }

    /// §782
    fn get_preamble_token(&mut self) -> Result<(), Jump> {
        loop {
            self.get_token()?;
            while self.cur_chr == SPAN_CODE && self.cur_cmd == TAB_MARK {
                self.get_token()?; // this token will be expanded once
                if self.cur_cmd > MAX_COMMAND {
                    self.expand()?;
                    self.get_token()?;
                }
            }
            if self.cur_cmd == ENDV {
                return self.fatal_error(b"(interwoven alignment preambles are not allowed)");
            }
            if self.cur_cmd == ASSIGN_GLUE && self.cur_chr == GLUE_BASE + TAB_SKIP_CODE {
                self.scan_optional_equals()?;
                self.scan_glue(GLUE_VAL)?;
                let origin = self.glue_origin.take();
                let g = crate::objs::Obj::Glue(self.new_glue_value(self.cur_glue, origin));
                if self.int_par(GLOBAL_DEFS_CODE) > 0 {
                    self.geq_define_obj(GLUE_BASE + TAB_SKIP_CODE, GLUE_REF, NULL, Some(g));
                } else {
                    self.eq_define_obj(GLUE_BASE + TAB_SKIP_CODE, GLUE_REF, NULL, Some(g))?;
                }
                continue;
            }
            return Ok(());
        }
    }

    /// §785
    pub(crate) fn align_peek(&mut self) -> Result<(), Jump> {
        loop {
            self.set_align_state(1_000_000);
            // §406: get the next non-blank non-call token.
            loop {
                self.get_x_or_protected()?;
                if self.cur_cmd != SPACER {
                    break;
                }
            }
            if self.cur_cmd == NO_ALIGN {
                self.scan_left_brace()?;
                self.new_save_level(NO_ALIGN_GROUP)?;
                if self.mode() == -VMODE {
                    self.normal_paragraph()?;
                }
            } else if self.cur_cmd == RIGHT_BRACE {
                self.fin_align()?;
            } else if self.cur_cmd == CAR_RET && self.cur_chr == CR_CR_CODE {
                continue; // ignore \crcr
            } else {
                self.init_row()?; // start a new row
                self.init_col()?; // start a new column and replace what we peeked at
            }
            return Ok(());
        }
    }

    /// §786
    fn init_row(&mut self) -> Result<(), Jump> {
        self.push_nest()?;
        self.set_mode((-HMODE - VMODE) - self.mode());
        if self.mode() == -HMODE {
            self.set_space_factor(0);
        } else {
            self.set_prev_depth(0);
        }
        let g = self.tabskip(0);
        self.tail_append(param_glue(g, TAB_SKIP_CODE));
        self.set_cur_align(Some(0));
        self.row_adjust_mut().clear(); // `cur_tail:=cur_head`
        self.init_span(0)
    }

    /// §787
    fn init_span(&mut self, p: usize) -> Result<(), Jump> {
        self.push_nest()?;
        if self.mode() == -HMODE {
            self.set_space_factor(1000);
        } else {
            self.set_prev_depth(IGNORE_DEPTH);
            self.normal_paragraph()?;
        }
        self.set_cur_span(p);
        Ok(())
    }

    /// §788
    fn init_col(&mut self) -> Result<(), Jump> {
        let cmd = self.cur_cmd;
        let Some(u) = self.edit_cur_column(|r| {
            r.extra_info = cmd;
            r.u.clone()
        }) else {
            return self.confusion(b"endv");
        };
        if cmd == OMIT {
            self.set_align_state(0);
        } else {
            self.back_input()?;
            self.begin_token_list(u, U_TEMPLATE)?;
        } // now `align_state=1000000`
        Ok(())
    }

    /// §791
    pub(crate) fn fin_col(&mut self) -> Result<bool, Jump> {
        let Some(cur) = self.cur_align() else {
            return self.confusion(b"endv");
        };
        if self.align_state() < 500_000 {
            self.fatal_error(b"(interwoven alignment preambles are not allowed)")?;
        }
        let extra = self.column(cur).extra_info;
        let mut p = (cur + 1 < self.column_count()).then_some(cur + 1);
        // §792: if the preamble list has been traversed, check that the
        // row has ended.
        if p.is_none() && extra < CR_CODE {
            if let Some(l) = self.cur_loop() {
                // §793: lengthen the preamble periodically.
                // §794: copy the templates from the column after `cur_loop`.
                // (a template is a value: the copy shares it)
                let r = self.column(l);
                let (u, v) = (r.u.clone(), r.v.clone());
                self.push_column(AlignRecord::new(u, v));
                self.set_cur_loop(Some(l + 1));
                let g = self.tabskip(l + 1);
                self.push_tabskip(g);
                p = Some(self.column_count() - 1);
            } else {
                self.print_err(b"Extra alignment tab has been changed to ");
                self.print_esc(b"cr");
                self.help(&[
                    b"You have given more \\span or & marks than there were",
                    b"in the preamble to the \\halign or \\valign now in progress.",
                    b"So I'll assume that you meant to type \\cr instead.",
                ]);
                self.edit_column(cur, |r| r.extra_info = CR_CODE);
                self.error()?;
            }
        }
        if self.column(cur).extra_info != SPAN_CODE {
            self.unsave()?;
            self.new_save_level(ALIGN_GROUP)?;
            // §796: package an unset box for the current column and record
            // its width.
            let list = core::mem::take(self.nodes_mut()).into_vec();
            let (u, w) = if self.mode() == -HMODE {
                // (the row's adjust list, `cur_tail`, is where the
                // entry's material migrates, §796)
                let mut migrated = Vec::new();
                let packed = self.hpack_full(list, Spec::NATURAL, Some(&mut migrated));
                if !migrated.is_empty() {
                    self.row_adjust_mut().extend(migrated);
                }
                let (st, sh) = (packed.total_stretch, packed.total_shrink);
                let b = packed.node;
                let w = b.width;
                ((b, st, sh), w)
            } else {
                let mut packed = self.vpack_call(list, Spec::NATURAL, 0)?;
                self.sync_box(&mut packed.node);
                let (st, sh) = (packed.total_stretch, packed.total_shrink);
                let b = packed.node;
                let w = b.height;
                ((b, st, sh), w)
            };
            let span = self.cur_span();
            let n = cur - span; // the span count, minus one
            if n == 0 {
                if w > self.column(cur).column.width {
                    self.edit_column(cur, |r| r.column.width = w);
                }
            } else {
                // §798: update width entry for spanned columns.
                if n > 255 {
                    self.confusion(b"256 spans")?; // this can happen, but won't
                }
                let n = u16::try_from(n).unwrap_or(u16::MAX);
                let spans = &self.column(span).column.spans;
                match spans.binary_search_by_key(&n, |s| s.0) {
                    Ok(k) => {
                        if spans[k].1 < w {
                            self.edit_column(span, |r| r.column.spans[k].1 = w);
                        }
                    }
                    Err(k) => self.edit_column(span, |r| r.column.spans.insert(k, (n, w))),
                }
            }
            let (b, st, sh) = u;
            // §659, §665: determine the stretch and shrink orders.
            let (so, ho) = (order(&st), order(&sh));
            let unset = Unset {
                width: b.width,
                height: b.height,
                depth: b.depth,
                span_count: u16::try_from(n).unwrap_or(0),
                stretch: st[so.index()],
                shrink: sh[ho.index()],
                stretch_order: so,
                shrink_order: ho,
                list: b.list,
                // (the box made an unset node: its place kept)
                sync: b.sync,
            };
            self.pop_nest();
            self.nodes_mut().push(Node::Unset(Box::new(unset)));
            // §795: copy the tabskip glue between columns.
            let g = self.tabskip(cur + 1);
            self.tail_append(param_glue(g, TAB_SKIP_CODE));
            if self.column(cur).extra_info >= CR_CODE {
                return Ok(true);
            }
            self.init_span(p.unwrap_or(cur + 1))?;
        }
        self.set_align_state(1_000_000);
        loop {
            self.get_x_or_protected()?;
            if self.cur_cmd != SPACER {
                break;
            }
        }
        self.set_cur_align(p);
        self.init_col()?;
        Ok(false)
    }

    /// §799
    pub(crate) fn fin_row(&mut self) -> Result<(), Jump> {
        let list = core::mem::take(self.nodes_mut()).into_vec();
        let row;
        if self.mode() == -HMODE {
            let b = self.hpack(list, Spec::NATURAL, None);
            self.pop_nest();
            row = unset_row(b);
            self.append_to_vlist(row);
            let mut adjust = core::mem::take(self.row_adjust_mut());
            if self.synctex_on() {
                let mut v = adjust.into_vec();
                self.sync_list(&mut v);
                adjust = partex_engine::nodelist::NodeList::from_vec(v);
            }
            self.nodes_mut().append(&mut adjust);
        } else {
            let b = self.vpack(list, Spec::NATURAL)?;
            self.pop_nest();
            let mut row = unset_row(b);
            self.sync_node(&mut row);
            self.nodes_mut().push(row);
            self.set_space_factor(1000);
        }
        self.begin_toks_at(EVERY_CR_LOC, EVERY_CR_TEXT)?;
        self.align_peek()
    }

    /// §800
    fn fin_align(&mut self) -> Result<(), Jump> {
        if self.cur_group() != ALIGN_GROUP {
            self.confusion(b"align1")?;
        }
        self.unsave()?; // that `align_group` was for individual entries
        if self.cur_group() != ALIGN_GROUP {
            self.confusion(b"align0")?;
        }
        self.unsave()?; // that `align_group` was for the whole alignment
        let o: Scaled = if self.level_mode(self.nest_ptr() - 1) == MMODE {
            self.dimen_par(DISPLAY_INDENT_CODE)
        } else {
            0
        };
        // §801: the templates are no longer needed.
        let level = self.take_align_level();
        let columns: Vec<Column> = level.columns.iter().map(|r| r.column.clone()).collect();
        let mut tabskips = level.tabskips.glue;
        self.set_save_ptr(self.save_ptr() - 2);
        let vertical = self.mode() != -VMODE;
        let pdftex = self.params.flavor == crate::params::Flavor::PdfTex;
        let params = align::Params {
            vertical,
            display: pdftex
                && !vertical
                && self.nest_ptr() > 0
                && self.level_mode(self.nest_ptr() - 1) == MMODE,
            pdftex,
            shift: o,
        };
        // §801–§803
        let pre = match align::preamble(columns, &mut tabskips, vertical) {
            Ok(pre) => pre,
            Err(c) => return self.confusion(c.0.as_bytes()),
        };
        // §804: package the preamble list, reporting on it as an
        // alignment's.
        let s = spec(self.saved(0), self.saved(1));
        self.pack_begin_line = -self.ml();
        let prototype = if vertical {
            let mut p = self.vpack_call(pre, s, MAX_DIMEN)?.node;
            align::prototype_widths(&mut p);
            p
        } else {
            self.hpack_preamble(pre, s).node
        };
        self.pack_begin_line = 0;
        // §805–§806: set the glue in all the unset boxes of the current
        // list.
        let list = core::mem::take(self.nodes_mut()).into_vec();
        let mut p = match align::set_rows(list, &prototype, &tabskips, params, |r| {
            self.hpack(alloc::vec![r], Spec::NATURAL, None)
        }) {
            Ok(p) => p,
            Err(c) => return self.confusion(c.0.as_bytes()),
        };
        self.pop_alignment();
        // (`SyncTeX`: what the alignment made now is placed here)
        self.sync_list(&mut p);
        // §812: insert the current list into its environment.
        let (pd, sf, lang) = (self.prev_depth(), self.space_factor(), self.clang());
        self.pop_nest();
        if self.mode() == MMODE {
            self.finish_display_alignment(p, pd)
        } else {
            self.set_prev_depth(pd);
            self.set_space_factor(sf);
            self.set_clang(lang);
            self.nodes_mut().extend(p);
            if self.mode() == VMODE {
                self.build_page()?;
            }
            Ok(())
        }
    }
}

/// §799: a finished row, as an unset node (`type(p):=unset_node;
/// glue_stretch(p):=0`).
fn unset_row(b: partex_engine::node::BoxNode) -> Node {
    Node::Unset(Box::new(Unset {
        width: b.width,
        height: b.height,
        depth: b.depth,
        list: b.list,
        // (the box made an unset node: its place kept)
        sync: b.sync,
        ..Unset::default()
    }))
}
