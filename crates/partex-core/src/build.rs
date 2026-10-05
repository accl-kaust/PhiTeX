//! Part 47: Building boxes and lists (§1055–§1135).

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use partex_engine::math::{Field, Item, Kind, Noad};
use partex_engine::node::{
    Adjust, BoxNode, Disc, GlueSpec, Ins, LeaderNode, Leaders, Mark, Node, Order,
};
use partex_engine::pack::Spec;

use crate::arith::{Scaled, zround};
use crate::display::tex_len;
use crate::host::Host;
use crate::mem::NULL;
use crate::nest::IGNORE_DEPTH;
use crate::nodes::*;
use crate::pack::spec;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;
use crate::xregs::reg_loc;

/// §1058: `chr_code`s of \hskip, \vskip and friends.
pub(crate) const FIL_CODE: i32 = 0;
pub(crate) const FILL_CODE: i32 = 1;
pub(crate) const SS_CODE: i32 = 2;
pub(crate) const FIL_NEG_CODE: i32 = 3;
pub(crate) const SKIP_CODE: i32 = 4;
pub(crate) const MSKIP_CODE: i32 = 5;
/// §1071: box contexts and `make_box` codes.
// (the box contexts are e-TeX's, whose registers go up to 32767)
pub(crate) use crate::web::{BOX_FLAG, GLOBAL_BOX_FLAG, LEADER_FLAG, SHIP_OUT_FLAG};
pub(crate) const BOX_CODE: i32 = 0;
pub(crate) const COPY_CODE: i32 = 1;
pub(crate) const LAST_BOX_CODE: i32 = 2;
pub(crate) const VSPLIT_CODE: i32 = 3;
pub(crate) const VTOP_CODE: i32 = 4;

/// §1091
pub(crate) fn norm_min(h: i32) -> i32 {
    h.clamp(1, 63)
}

/// §162: the constant glue specifications `fil_glue`, `fill_glue`,
/// `ss_glue` and `fil_neg_glue`.
pub(crate) const FIL_GLUE: GlueSpec = GlueSpec {
    stretch: 0o200000,
    stretch_order: Order::Fil,
    ..GlueSpec::ZERO_GLUE
}
.copy();
pub(crate) const FILL_GLUE: GlueSpec = GlueSpec {
    stretch: 0o200000,
    stretch_order: Order::Fill,
    ..GlueSpec::ZERO_GLUE
}
.copy();
pub(crate) const SS_GLUE: GlueSpec = GlueSpec {
    stretch: 0o200000,
    stretch_order: Order::Fil,
    shrink: 0o200000,
    shrink_order: Order::Fil,
    ..GlueSpec::ZERO_GLUE
}
.copy();
pub(crate) const FIL_NEG_GLUE: GlueSpec = GlueSpec {
    stretch: -0o200000,
    stretch_order: Order::Fil,
    ..GlueSpec::ZERO_GLUE
}
.copy();

/// Append `src` to `dst`, keeping glyph runs canonical (and, with `t`,
/// the characters' origins: `srcmap.rs`).
pub(crate) fn append_list(
    dst: &mut partex_engine::nodelist::NodeList,
    src: Vec<Node>,
    mut t: Option<&mut partex_engine::origin::OrgTable>,
) {
    let mut src = src.into_iter();
    for n in src.by_ref() {
        match n {
            Node::Glyphs(g) => match t.as_deref_mut() {
                Some(t) => {
                    for (i, &c) in g.chars().iter().enumerate() {
                        let o = partex_engine::origin::char_org(t, &g, i);
                        dst.push_char_org(g.font, c, o, t);
                    }
                }
                None => {
                    for &c in g.chars() {
                        dst.push_char(g.font, c);
                    }
                }
            },
            n => {
                dst.push(n);
                break;
            }
        }
    }
    dst.extend(src);
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Append `src` to the current list ([`append_list`]).
    pub(crate) fn append_nodes(&mut self, mut src: Vec<Node>) {
        self.sync_list(&mut src);
        // (the current list read and written, as `nodes_mut`)
        let _ = self.nodes_mut();
        let t = self.org.as_deref_mut().map(|o| &mut o.table);
        append_list(&mut self.cur_list.list, src, t);
    }

    /// A single glyph node `n` (made by `new_character`) given origin `o`.
    pub(crate) fn with_org(&mut self, mut n: Node, o: partex_engine::origin::Org) -> Node {
        if let Node::Glyphs(g) = &mut n {
            let h = self.org_handle(o);
            g.set_org(h);
        }
        n
    }

    /// §1060
    pub(crate) fn append_glue(&mut self) -> Result<(), Jump> {
        let (spec, subtype) = self.scan_appended_glue()?;
        self.tail_append(Node::Glue {
            spec,
            subtype,
            sync: partex_engine::origin::Side(0),
        });
        Ok(())
    }

    /// §1060: the glue of `\hskip`, `\vskip` and friends (`cur_chr`).
    fn scan_appended_glue(&mut self) -> Result<(GlueSpec, u8), Jump> {
        let s = self.cur_chr;
        let spec = match s {
            FIL_CODE => FIL_GLUE,
            FILL_CODE => FILL_GLUE,
            SS_CODE => SS_GLUE,
            FIL_NEG_CODE => FIL_NEG_GLUE,
            SKIP_CODE => {
                self.scan_glue(GLUE_VAL)?;
                self.cur_glue
            }
            _ => {
                self.scan_glue(MU_VAL)?;
                self.cur_glue
            }
        };
        let subtype = if s == MSKIP_CODE { subtype(MU_GLUE) } else { 0 };
        Ok((spec, subtype))
    }

    /// §1061
    pub(crate) fn append_kern(&mut self) -> Result<(), Jump> {
        let s = self.cur_chr;
        self.scan_dimen(s == MU_GLUE, false, false)?;
        self.tail_append(Node::Kern {
            width: self.cur_val,
            subtype: u8::try_from(s).unwrap_or(0),
            sync: partex_engine::origin::Side(0),
        });
        Ok(())
    }

    /// §1064
    pub(crate) fn off_save(&mut self) -> Result<(), Jump> {
        if self.cur_group() == BOTTOM_LEVEL {
            // §1066: drop current token and complain that it was unmatched.
            self.print_err(b"Extra ");
            self.print_cmd_chr(self.cur_cmd, self.cur_chr);
            self.help(&[b"Things are pretty mixed up, but I think the worst is over."]);
            return self.error();
        }
        self.back_input()?;
        self.print_err(b"Missing ");
        // §1065: prepare to insert a token that matches `cur_group`, and
        // print what it is.
        let p = match self.cur_group() {
            SEMI_SIMPLE_GROUP => {
                self.print_esc(b"endgroup");
                self.tok_from(&[CS_TOKEN_FLAG + FROZEN_END_GROUP])
            }
            MATH_SHIFT_GROUP => {
                self.print_char(b'$');
                self.tok_from(&[MATH_SHIFT_TOKEN + i32::from(b'$')])
            }
            MATH_LEFT_GROUP => {
                self.print_esc(b"right.");
                self.tok_from(&[CS_TOKEN_FLAG + FROZEN_RIGHT, OTHER_TOKEN + i32::from(b'.')])
            }
            _ => {
                self.print_char(b'}');
                self.tok_from(&[RIGHT_BRACE_TOKEN + i32::from(b'}')])
            }
        };
        self.print_str(b" inserted");
        self.ins_list(p)?;
        self.help(&[
            b"I've inserted something that you may have forgotten.",
            b"(See the <inserted text> above.)",
            b"With luck, this will get me unwedged. But if you",
            b"really didn't forget anything, try typing `2' now; then",
            b"my insertion and my current dilemma will both disappear.",
        ]);
        self.error()
    }

    /// §1068
    pub(crate) fn handle_right_brace(&mut self) -> Result<(), Jump> {
        match self.cur_group() {
            SIMPLE_GROUP => self.unsave(),
            BOTTOM_LEVEL => {
                self.print_err(b"Too many }'s");
                self.help(&[
                    b"You've closed more groups than you opened.",
                    b"Such booboos are generally harmless, so keep going.",
                ]);
                self.error()
            }
            SEMI_SIMPLE_GROUP | MATH_SHIFT_GROUP | MATH_LEFT_GROUP => self.extra_right_brace(),
            // §1085
            HBOX_GROUP => self.package(0),
            ADJUSTED_HBOX_GROUP => {
                self.adjust = Some(Vec::new());
                self.package(0)
            }
            VBOX_GROUP => {
                self.end_graf()?;
                self.package(0)
            }
            VTOP_GROUP => {
                self.end_graf()?;
                self.package(VTOP_CODE)
            }
            INSERT_GROUP => self.finish_insert(),       // §1100
            OUTPUT_GROUP => self.resume_page_builder(), // §1026
            DISC_GROUP => self.build_discretionary(),   // §1118
            ALIGN_GROUP => {
                // §1132
                self.back_input()?;
                self.cur_tok = CS_TOKEN_FLAG + FROZEN_CR;
                self.print_err(b"Missing ");
                self.print_esc(b"cr");
                self.print_str(b" inserted");
                self.help(&[b"I'm guessing that you meant to end an alignment here."]);
                self.ins_error()
            }
            NO_ALIGN_GROUP => {
                // §1133
                self.end_graf()?;
                self.unsave()?;
                self.align_peek()
            }
            VCENTER_GROUP => self.finish_vcenter(), // §1168
            MATH_CHOICE_GROUP => self.build_choices(), // §1173
            MATH_GROUP => self.finish_math_group(), // §1186
            _ => self.confusion(b"rightbrace"),
        }
    }

    /// §1069
    fn extra_right_brace(&mut self) -> Result<(), Jump> {
        self.print_err(b"Extra }, or forgotten ");
        match self.cur_group() {
            SEMI_SIMPLE_GROUP => self.print_esc(b"endgroup"),
            MATH_SHIFT_GROUP => self.print_char(b'$'),
            MATH_LEFT_GROUP => self.print_esc(b"right"),
            _ => {}
        }
        self.help(&[
            b"I've deleted a group-closing symbol because it seems to be",
            b"spurious, as in `$x}$'. But perhaps the } is legitimate and",
            b"you forgot something else, as in `\\hbox{$x}'. In such cases",
            b"the way to recover is to insert both the forgotten and the",
            b"deleted material, e.g., by typing `I$}'.",
        ]);
        self.error()?;
        self.set_align_state(self.align_state() + 1);
        Ok(())
    }

    /// §1075
    pub(crate) fn box_end(&mut self, box_context: i32) -> Result<(), Jump> {
        let mut cur_box = self.cur_box.take();
        // (`SyncTeX`: a box made now is placed here)
        if let Some(b) = &mut cur_box {
            self.sync_node(b);
        }
        if box_context < BOX_FLAG {
            // §1076: append box `cur_box` to the current list, shifted by
            // `box_context`.
            if let Some(Node::Box(mut b)) = cur_box {
                if b.shift != box_context {
                    let m = Arc::make_mut(&mut b);
                    m.shift = box_context;
                    m.reversion();
                }
                if self.mode().abs() == VMODE {
                    self.append_to_vlist(Node::Box(b));
                    if let Some(mut adjust) = self.adjust.take() {
                        self.sync_list(&mut adjust);
                        self.nodes_mut().extend(adjust);
                    }
                    if self.mode() > 0 {
                        self.build_page()?;
                    }
                } else if self.mode().abs() == HMODE {
                    self.set_space_factor(1000);
                    self.nodes_mut().push(Node::Box(b));
                } else {
                    let mut n = Noad::new(Kind::Ord);
                    n.nucleus = Field::Box(b);
                    self.mlist_mut().push(Item::Noad(Box::new(n)));
                }
            }
        } else if box_context < SHIP_OUT_FLAG {
            // §1077: store `cur_box` in a box register.
            let obj = match cur_box {
                Some(Node::Box(b)) => Some(crate::objs::Obj::Box(b)),
                _ => None,
            };
            if box_context < GLOBAL_BOX_FLAG {
                let l = reg_loc(BOX_VAL, box_context - BOX_FLAG);
                self.eq_define_obj(l, BOX_REF, NULL, obj)?;
            } else {
                let l = reg_loc(BOX_VAL, box_context - GLOBAL_BOX_FLAG);
                self.geq_define_obj(l, BOX_REF, NULL, obj);
            }
        } else if let Some(cur_box) = cur_box {
            if box_context > SHIP_OUT_FLAG {
                // §1078: append a new leader node that uses `cur_box`.
                self.get_nonblank_nonrelax_noncall()?;
                if (self.cur_cmd == HSKIP && self.mode().abs() != VMODE)
                    || (self.cur_cmd == VSKIP && self.mode().abs() == VMODE)
                {
                    let (spec, _) = self.scan_appended_glue()?;
                    let kind = match box_context - LEADER_FLAG {
                        0 => Leaders::Aligned,
                        1 => Leaders::Centered,
                        _ => Leaders::Expanded,
                    };
                    self.tail_append(Node::Leaders(Box::new(LeaderNode {
                        spec,
                        kind,
                        leader: cur_box,
                        sync: partex_engine::origin::Side(0),
                    })));
                } else {
                    self.print_err(b"Leaders not followed by proper glue");
                    self.help(&[
                        b"You should say `\\leaders <box or rule><hskip or vskip>'.",
                        b"I found the <box or rule>, but there's no suitable",
                        b"<hskip or vskip>, so I'm ignoring these leaders.",
                    ]);
                    self.back_error()?;
                }
            } else if let Node::Box(b) = cur_box {
                self.ship_out(&b)?;
            }
        }
        Ok(())
    }

    /// §1079
    pub(crate) fn begin_box(&mut self, box_context: i32) -> Result<(), Jump> {
        match self.cur_chr {
            BOX_CODE => {
                self.scan_register_num()?;
                // the box becomes void, at the same level
                self.cur_box = self.take_box(self.cur_val).map(Node::Box);
            }
            COPY_CODE => {
                self.scan_register_num()?;
                self.cur_box = self.box_reg(self.cur_val).cloned().map(Node::Box);
                // (`SyncTeX`: a copy's rules are made now)
                if self.synctex_on()
                    && let Some(mut b) = self.cur_box.take()
                {
                    self.sync_copied(core::slice::from_mut(&mut b));
                    self.cur_box = Some(b);
                }
            }
            LAST_BOX_CODE => {
                // §1080: if the current list ends with a box node, delete it
                // from the list and make `cur_box` point to it; otherwise
                // set `cur_box:=null`.
                self.cur_box = None;
                if self.mode().abs() == MMODE {
                    self.you_cant();
                    self.help(&[b"Sorry; this \\lastbox will be void."]);
                    self.error()?;
                } else if self.mode() == VMODE && self.list_is_empty() {
                    self.you_cant();
                    self.help(&[
                        b"Sorry...I usually can't take things from the current page.",
                        b"This \\lastbox will therefore be void.",
                    ]);
                    self.error()?;
                } else if let Some(Node::Box(_)) = self.nodes().last() {
                    // §1081: remove the last box (a box in a
                    // discretionary's replacement is not the last node).
                    if let Some(Node::Box(mut b)) = self.nodes_mut().pop() {
                        if b.shift != 0 {
                            let m = Arc::make_mut(&mut b);
                            m.shift = 0;
                            m.reversion();
                        }
                        self.cur_box = Some(Node::Box(b));
                    }
                }
            }
            VSPLIT_CODE => {
                // §1082: split off part of a vertical box, make `cur_box`
                // point to it.
                self.scan_register_num()?;
                let n = self.cur_val;
                if !self.scan_keyword(b"to")? {
                    self.print_err(b"Missing `to' inserted");
                    self.help(&[
                        b"I'm working on `\\vsplit<box number> to <dimen>';",
                        b"will look for the <dimen> next.",
                    ]);
                    self.error()?;
                }
                self.scan_normal_dimen()?;
                self.cur_box = self.vsplit(n, self.cur_val)?.map(Node::Box);
            }
            _ => {
                // §1083: initiate the construction of an hbox or vbox, then
                // `return`.
                let mut k = self.cur_chr - VTOP_CODE;
                self.set_saved(0, box_context);
                if k == HMODE {
                    if box_context < BOX_FLAG && self.mode().abs() == VMODE {
                        self.scan_spec(ADJUSTED_HBOX_GROUP, true)?;
                    } else {
                        self.scan_spec(HBOX_GROUP, true)?;
                    }
                } else {
                    if k == VMODE {
                        self.scan_spec(VBOX_GROUP, true)?;
                    } else {
                        self.scan_spec(VTOP_GROUP, true)?;
                        k = VMODE;
                    }
                    self.normal_paragraph()?;
                }
                self.push_nest()?;
                self.set_mode(-k);
                if k == VMODE {
                    self.set_prev_depth(IGNORE_DEPTH);
                    self.begin_toks_at(EVERY_VBOX_LOC, EVERY_VBOX_TEXT)?;
                } else {
                    self.set_space_factor(1000);
                    self.begin_toks_at(EVERY_HBOX_LOC, EVERY_HBOX_TEXT)?;
                }
                return Ok(());
            }
        }
        self.box_end(box_context) // in simple cases, we use the box immediately
    }

    /// §1084: the next input should specify a box or perhaps a rule.
    pub(crate) fn scan_box(&mut self, box_context: i32) -> Result<(), Jump> {
        self.get_nonblank_nonrelax_noncall()?;
        if self.cur_cmd == MAKE_BOX {
            self.begin_box(box_context)
        } else if box_context >= LEADER_FLAG && (self.cur_cmd == HRULE || self.cur_cmd == VRULE) {
            self.cur_box = Some(self.scan_rule_spec()?);
            self.box_end(box_context)
        } else {
            self.print_err(b"A <box> was supposed to be here");
            self.help(&[
                b"I was expecting to see \\hbox or \\vbox or \\copy or \\box or",
                b"something like that. So you might find something missing in",
                b"your output. But keep trying; you can fix this later.",
            ]);
            self.back_error()
        }
    }

    /// §1086
    fn package(&mut self, c: i32) -> Result<(), Jump> {
        let d = self.dimen_par(BOX_MAX_DEPTH_CODE);
        self.unsave()?;
        self.set_save_ptr(self.save_ptr() - 3);
        let list = core::mem::take(self.nodes_mut());
        let spec = spec(self.saved(1), self.saved(2));
        let b = if self.mode() == -HMODE {
            let mut adjust = self.adjust.take();
            let b = self.hpack(list.into_vec(), spec, adjust.as_mut());
            self.adjust = adjust;
            b
        } else {
            let mut b = self.vpackage(list.into_vec(), spec, d)?;
            if c == VTOP_CODE {
                // §1087: readjust the height and depth of `cur_box`, for
                // \vtop.
                let h = match b.list.first() {
                    Some(Node::Box(p)) => p.height,
                    Some(Node::Rule { height, .. }) => *height,
                    _ => 0,
                };
                b.depth = b.depth - h + b.height;
                b.height = h;
            }
            b
        };
        self.cur_box = Some(Node::Box(b.share()));
        self.pop_nest();
        self.box_end(self.saved(0))
    }

    /// §1091
    pub(crate) fn new_graf(&mut self, indented: bool) -> Result<(), Jump> {
        self.set_pg(0);
        if self.mode() == VMODE || !self.list_is_empty() {
            let g = self.new_param_glue(PAR_SKIP_CODE);
            self.tail_append(g);
        }
        self.push_nest()?;
        self.set_mode(HMODE);
        self.set_space_factor(1000);
        self.set_cur_lang();
        self.set_clang(self.hyph.cur_lang);
        self.set_pg(
            (norm_min(self.int_par(LEFT_HYPHEN_MIN_CODE)) * 0o100
                + norm_min(self.int_par(RIGHT_HYPHEN_MIN_CODE)))
                * 0o200000
                + self.hyph.cur_lang,
        );
        if indented {
            let mut indent = self.indent_box();
            self.sync_node(&mut indent);
            self.nodes_mut().push(indent);
        }
        self.begin_toks_at(EVERY_PAR_LOC, EVERY_PAR_TEXT)?;
        if self.nest_ptr() == 1 {
            // (a paragraph's start: the next candidate boundary, in
            // machine mode)
            self.par_start = self.stop_at_candidate && crate::machine::clean_cuts();
            if T::VALUES
                && self.defer_page
                && self.commands - self.step_began >= crate::run::DEFER_PAGE_AFTER
            {
                // (the step ran commands before the paragraph began, a
                // picture built in vertical mode before its `\leavevmode`:
                // the page builder is at the next command, a step of its
                // own, and they read none of the page's state; the step
                // that begins there takes the paragraph's `mode_line`, the
                // line it begins on, as its own)
                self.page_pending = true;
                self.graf_stop = true;
            } else {
                self.build_page()?; // put `par_skip` glue on current page
            }
        }
        Ok(())
    }

    /// §1091: an empty box of width `\parindent`.
    fn indent_box(&self) -> Node {
        Node::Box(
            (BoxNode {
                width: self.dimen_par(PAR_INDENT_CODE),
                ..BoxNode::default()
            })
            .share(),
        )
    }

    /// §1093
    pub(crate) fn indent_in_hmode(&mut self) {
        if self.cur_chr > 0 {
            // \indent
            let mut p = self.indent_box();
            self.sync_node(&mut p);
            if self.mode().abs() == HMODE {
                self.set_space_factor(1000);
                self.nodes_mut().push(p);
            } else if let Node::Box(b) = p {
                let mut q = Noad::new(Kind::Ord);
                q.nucleus = Field::Box(b);
                self.mlist_mut().push(Item::Noad(Box::new(q)));
            }
        }
    }

    /// §1095
    pub(crate) fn head_for_vmode(&mut self) -> Result<(), Jump> {
        if self.mode() < 0 {
            if self.cur_cmd != HRULE {
                return self.off_save();
            }
            self.print_err(b"You can't use `");
            self.print_esc(b"hrule");
            self.print_str(b"' here except with leaders");
            self.help(&[
                b"To put a horizontal rule in an hbox or an alignment,",
                b"you should use \\leaders or \\hrulefill (see The TeXbook).",
            ]);
            return self.error();
        }
        self.back_input()?;
        self.cur_tok = self.par_token;
        self.back_input()?;
        self.cur_input.index = INSERTED; // `token_type:=inserted`
        Ok(())
    }

    /// §1096
    pub(crate) fn end_graf(&mut self) -> Result<(), Jump> {
        if self.mode() == HMODE {
            if self.list_is_empty() {
                self.pop_nest(); // null paragraphs are ignored
            } else {
                self.line_break(false)?;
                // (a paragraph broken into lines ends the open window at
                // the next boundary, DESIGN 4.3 item 1; not a null one:
                // LaTeX's paragraph hooks begin each paragraph with one)
                self.window_event(crate::run::WindowEvent::ParEnd);
            }
            self.lr_save_mut().clear();
            self.normal_paragraph()?;
            self.set_error_count(0);
        }
        Ok(())
    }

    /// §1099
    pub(crate) fn begin_insert_or_adjust(&mut self) -> Result<(), Jump> {
        if self.cur_cmd == VADJUST {
            self.cur_val = 255;
        } else {
            self.scan_eight_bit_int()?;
            if self.cur_val == 255 {
                self.print_err(b"You can't ");
                self.print_esc(b"insert");
                self.print_int(255);
                self.help(&[b"I'm changing to \\insert0; box 255 is special."]);
                self.error()?;
                self.cur_val = 0;
            }
        }
        self.set_saved(0, self.cur_val);
        self.set_save_ptr(self.save_ptr() + 1);
        if self.params.flavor == crate::params::Flavor::PdfTex {
            // pdfTeX: `\vadjust pre`.
            let pre = self.cur_cmd == VADJUST && self.scan_keyword(b"pre")?;
            self.set_saved(0, i32::from(pre));
            self.set_save_ptr(self.save_ptr() + 1);
        }
        self.new_save_level(INSERT_GROUP)?;
        self.scan_left_brace()?;
        self.normal_paragraph()?;
        self.push_nest()?;
        self.set_mode(-VMODE);
        self.set_prev_depth(IGNORE_DEPTH);
        Ok(())
    }

    /// §1100: `insert_group` in `handle_right_brace`.
    fn finish_insert(&mut self) -> Result<(), Jump> {
        self.end_graf()?;
        let q = self.glue_par(SPLIT_TOP_SKIP_CODE);
        let d = self.dimen_par(SPLIT_MAX_DEPTH_CODE);
        let f = self.int_par(FLOATING_PENALTY_CODE);
        self.unsave()?;
        let pre = if self.params.flavor == crate::params::Flavor::PdfTex {
            self.set_save_ptr(self.save_ptr() - 1);
            self.saved(0) != 0
        } else {
            false
        };
        self.set_save_ptr(self.save_ptr() - 1);
        // now `saved(0)` is the insertion number, or 255 for \vadjust
        let list = core::mem::take(self.nodes_mut());
        let p = self.vpack(list.into_vec(), Spec::NATURAL)?;
        self.pop_nest();
        if self.saved(0) < 255 {
            self.tail_append(Node::Ins(Box::new(Ins {
                number: u8::try_from(self.saved(0)).unwrap_or(0),
                height: p.height + p.depth,
                split_top: q,
                split_max_depth: d,
                float_cost: f,
                list: p.list,
            })));
        } else {
            self.tail_append(Node::Adjust(Box::new(Adjust { pre, list: p.list })));
        }
        if self.nest_ptr() == 0 {
            self.build_page()?;
        }
        Ok(())
    }

    /// §1026: resume the page builder after an output routine has come to
    /// an end.
    fn resume_page_builder(&mut self) -> Result<(), Jump> {
        if self.cur_input.loc != NULL
            || (self.cur_input.index != OUTPUT_TEXT && self.cur_input.index != BACKED_UP)
        {
            // §1027: recover from an unbalanced output routine.
            self.print_err(b"Unbalanced output routine");
            self.help(&[
                b"Your sneaky output routine has problematic {'s and/or }'s.",
                b"I can't handle that very well; good luck.",
            ]);
            self.error()?;
            loop {
                self.get_token()?;
                if self.cur_input.loc == NULL {
                    break;
                }
            } // loops forever if reading from a file, since `null=min_halfword<=0`
        }
        self.end_token_list()?; // conserve stack space in case more outputs are triggered
        self.end_graf()?;
        self.unsave()?;
        self.set_output_active(false);
        self.set_insert_penalties(0);
        // §1028: ensure that box 255 is empty after output.
        if self.box_reg(255).is_some() {
            self.print_err(b"Output routine didn't use all of ");
            self.print_esc(b"box");
            self.print_int(255);
            self.help(&[
                b"Your \\output commands should empty \\box255,",
                b"e.g., by saying `\\shipout\\box255'.",
                b"Proceed; I'll discard its present contents.",
            ]);
            self.box_error(255)?;
        }
        // The current list goes after heldover insertions, and both go
        // before heldover contributions.
        let mut list = core::mem::take(self.nodes_mut());
        let mut held = self.page_take_list();
        held.append(&mut list);
        self.page_discards_mut().clear();
        self.pop_nest();
        // (the output routine's call ends: its result is what goes in
        // front of the contributions)
        self.output_end(&held);
        if !held.is_empty() {
            let contrib = self.contrib();
            let mut rest = core::mem::replace(contrib, held);
            contrib.append(&mut rest);
        }
        self.build_page()
    }

    /// §1101
    pub(crate) fn make_mark(&mut self) -> Result<(), Jump> {
        let class = if self.cur_chr == 0 {
            0
        } else {
            self.scan_register_num()?;
            self.cur_val
        };
        self.scan_toks(false, true)?;
        let tokens = self.take_def();
        self.tail_append(Node::Mark(Box::new(Mark { class, tokens })));
        Ok(())
    }

    /// §1103
    pub(crate) fn append_penalty(&mut self) -> Result<(), Jump> {
        self.scan_int()?;
        self.tail_append(Node::Penalty(self.cur_val));
        if self.mode() == VMODE {
            self.build_page()?;
        }
        Ok(())
    }

    /// §1105
    pub(crate) fn delete_last(&mut self) -> Result<(), Jump> {
        if self.mode() == VMODE && self.list_is_empty() {
            // §1106: apologize for inability to do the operation now,
            // unless \unskip follows non-glue.
            if self.cur_chr != GLUE_NODE || self.page_last_glue().is_some() {
                self.you_cant();
                self.help(&[
                    b"Sorry...I usually can't take things from the current page.",
                    b"Try `I\\vskip-\\lastskip' instead.",
                ]);
                if self.cur_chr == KERN_NODE {
                    self.help_line[0] = b"Try `I\\kern-\\lastkern' instead.";
                } else if self.cur_chr != GLUE_NODE {
                    self.help_line[0] = b"Perhaps you can make the output routine do it.";
                }
                self.error()?;
            }
            return Ok(());
        }
        // A node in a discretionary's replacement stays (§1105).
        let in_disc = matches!(self.tail_item(), Some(Node::Disc(d)) if !d.replace.is_empty());
        if !in_disc
            && self
                .tail_node()
                .is_some_and(|n| node_type(n) == self.cur_chr)
        {
            self.pop_tail();
        }
        Ok(())
    }

    /// §1110
    pub(crate) fn unpackage(&mut self) -> Result<(), Jump> {
        let c = self.cur_chr;
        if c > COPY_CODE {
            // e-TeX: \pagediscards, \splitdiscards.
            let list = if c == LAST_BOX_CODE {
                core::mem::take(self.page_discards_mut()).into_vec()
            } else {
                core::mem::take(self.split_discards_mut()).into_vec()
            };
            self.append_nodes(without_margin_kerns(list));
            return Ok(());
        }
        self.scan_register_num()?;
        let Some(p) = self.box_reg(self.cur_val).cloned() else {
            return Ok(());
        };
        let m = self.mode().abs();
        if m == MMODE || (m == VMODE && !p.vertical) || (m == HMODE && p.vertical) {
            self.print_err(b"Incompatible list can't be unboxed");
            self.help(&[
                b"Sorry, Pandora. (You sneaky devil.)",
                b"I refuse to unbox an \\hbox in vertical mode or vice versa.",
                b"And I can't open any boxes in math mode.",
            ]);
            return self.error();
        }
        let mut list = if let Some(open) = self.unsealed_box(&p) {
            // (a sealed line: its contents, read)
            if c != COPY_CODE {
                let _ = self.take_box(self.cur_val);
            }
            open.list
        } else if c == COPY_CODE {
            p.list.clone()
        } else {
            drop(p);
            let b = self.take_box(self.cur_val).unwrap_or_default();
            Arc::try_unwrap(b).map_or_else(|b| b.list.clone(), |b| b.list)
        };
        if c == COPY_CODE {
            // (`SyncTeX`: a copy's rules are made now)
            self.sync_copied(&mut list);
        }
        self.append_nodes(without_margin_kerns(list));
        Ok(())
    }

    /// §1113
    pub(crate) fn append_italic_correction(&mut self) {
        let (f, c) = match self.tail_node() {
            Some(Node::Glyphs(g)) => (i32::from(g.font.0), g.chars()[g.chars().len() - 1]),
            Some(Node::Ligature(l)) => (i32::from(l.font.0), l.ch),
            _ => return,
        };
        let italic = self.char_metrics(f, i32::from(c)).italic;
        self.tail_append(Node::Kern {
            width: italic,
            subtype: subtype(EXPLICIT),
            sync: partex_engine::origin::Side(0),
        });
    }

    /// §1117
    pub(crate) fn append_discretionary(&mut self) -> Result<(), Jump> {
        let mut d = Disc::default();
        if self.cur_chr == 1 {
            // (`\-`'s hyphen: synthesized, the command's range)
            let o = self.char_num_org();
            let f = self.cur_font();
            self.font_read(f, crate::track::font::HYPHEN_CHAR);
            let c = self.fonts.hyphen_char[crate::fonts::fx(f)];
            if (0..256).contains(&c)
                && let Some(p) = self.new_character(f, c)?
            {
                let p = self.with_org(p, o);
                d.pre.push(p);
            }
            self.tail_append(Node::Disc(Box::new(d)));
        } else {
            self.tail_append(Node::Disc(Box::new(d)));
            self.set_save_ptr(self.save_ptr() + 1);
            self.set_saved(-1, 0);
            self.new_save_level(DISC_GROUP)?;
            self.scan_left_brace()?;
            self.push_nest()?;
            self.set_mode(-HMODE);
            self.set_space_factor(1000);
        }
        Ok(())
    }

    /// Change the discretionary at the end of the current list (in a
    /// list of values, the node is made again).
    fn tail_disc<R>(&mut self, f: impl FnOnce(&mut Disc) -> R) -> Option<R> {
        if self.mode().abs() == MMODE {
            return match self.mlist_mut().last_mut() {
                Some(Item::Node(Node::Disc(d))) => Some(f(d)),
                _ => None,
            };
        }
        if !matches!(self.nodes().last(), Some(Node::Disc(_))) {
            return None;
        }
        self.nodes_mut().edit_last(|n| match n {
            Node::Disc(d) => f(d),
            _ => unreachable!(),
        })
    }

    /// §1119
    fn build_discretionary(&mut self) -> Result<(), Jump> {
        self.unsave()?;
        // §1121: prune the current list, if necessary, until it contains
        // only char_node, kern_node, hlist_node, vlist_node, rule_node, and
        // ligature_node items.
        let mut p = core::mem::take(self.nodes_mut()).into_vec();
        // (`XeTeX`: and native words and glyphs)
        if let Some(bad) = p.iter().position(|n| {
            !matches!(
                n,
                Node::Glyphs(_)
                    | Node::Kern { .. }
                    | Node::Box(_)
                    | Node::Rule { .. }
                    | Node::Ligature(_)
            ) && !matches!(
                n,
                Node::Whatsit(w) if matches!(
                    **w,
                    partex_engine::node::Whatsit::NativeWord(_)
                        | partex_engine::node::Whatsit::Glyph(_)
                )
            )
        }) {
            self.print_err(b"Improper discretionary list");
            self.help(&[b"Discretionary lists must contain only boxes and kerns."]);
            self.error()?;
            self.begin_diagnostic();
            self.print_nl(b"The following discretionary sublist has been deleted:");
            let rest = p.split_off(bad);
            self.show_box(&rest);
            self.end_diagnostic(true);
        }
        // done:
        let mut n = tex_len(&p);
        self.pop_nest();
        match self.saved(-1) {
            0 => {
                self.tail_disc(|d| d.pre = p);
            }
            1 => {
                self.tail_disc(|d| d.post = p);
            }
            _ => {
                // §1120: attach list `p` to the current list, and record
                // its length; then finish up and `return`.
                if n > 0 && self.mode().abs() == MMODE {
                    self.print_err(b"Illegal math ");
                    self.print_esc(b"discretionary");
                    self.help(&[
                        b"Sorry: The third part of a discretionary break must be",
                        b"empty, in math formulas. I had to delete your third part.",
                    ]);
                    n = 0;
                    self.error()?;
                } else {
                    self.tail_disc(|d| d.replace = p);
                }
                if n > MAX_QUARTERWORD {
                    // tex.web keeps the list but not its length.
                    if let Some(replaced) = self.tail_disc(|d| core::mem::take(&mut d.replace)) {
                        self.append_nodes(replaced);
                    }
                    self.print_err(b"Discretionary list is too long");
                    self.help(&[
                        b"Wow---I never thought anybody would tweak me here.",
                        b"You can't seriously need such a huge discretionary list?",
                    ]);
                    self.error()?;
                }
                self.set_save_ptr(self.save_ptr() - 1);
                return Ok(());
            }
        }
        self.set_saved(-1, self.saved(-1) + 1);
        self.new_save_level(DISC_GROUP)?;
        self.scan_left_brace()?;
        self.push_nest()?;
        self.set_mode(-HMODE);
        self.set_space_factor(1000);
        Ok(())
    }

    /// §1123
    pub(crate) fn make_accent(&mut self) -> Result<(), Jump> {
        // (the accent's origin: synthesized, `\accent`'s range)
        let accent_org = self.char_num_org();
        self.scan_char_num()?;
        let mut f = self.cur_font();
        let Some(p) = self.new_character(f, self.cur_val)? else {
            return Ok(());
        };
        let mut p = self.with_org(p, accent_org);
        let x = self.font_param(X_HEIGHT_CODE, f);
        let s = f64::from(self.font_param(SLANT_CODE, f)) / 65536.0;
        let a = self.char_metrics(f, self.cur_val).width;
        self.do_assignments()?;
        // §1124: create a character node `q` for the next character, but
        // set `q:=null` if problems arise.
        let mut q = None;
        f = self.cur_font();
        if self.cur_cmd == LETTER || self.cur_cmd == OTHER_CHAR || self.cur_cmd == CHAR_GIVEN {
            let o = self.char_org();
            q = self
                .new_character(f, self.cur_chr)?
                .map(|n| (n, self.cur_chr, o));
        } else if self.cur_cmd == CHAR_NUM {
            let o = self.char_num_org();
            self.scan_char_num()?;
            q = self
                .new_character(f, self.cur_val)?
                .map(|n| (n, self.cur_val, o));
        } else {
            self.back_input()?;
        }
        if let Some((q, qc, qo)) = q {
            let q = self.with_org(q, qo);
            // §1125: append the accent with appropriate kerns, then set
            // `p:=q`.
            let t = f64::from(self.font_param(SLANT_CODE, f)) / 65536.0;
            let g = self.char_metrics(f, qc);
            let (w, h) = (g.width, g.height);
            if h != x {
                // the accent must be shifted up or down
                let mut b = self.hpack(alloc::vec![p], Spec::NATURAL, None);
                b.shift = x - h;
                p = Node::Box(b.share());
            }
            let delta: Scaled =
                zround(f64::from(w - a) / 2.0 + f64::from(h) * t - f64::from(x) * s);
            self.append_nodes(alloc::vec![
                Node::Kern {
                    width: delta,
                    subtype: subtype(ACC_KERN),
                    sync: partex_engine::origin::Side(0),
                },
                p,
                Node::Kern {
                    width: -a - delta,
                    subtype: subtype(ACC_KERN),
                    sync: partex_engine::origin::Side(0),
                },
            ]);
            p = q;
        }
        self.append_nodes(alloc::vec![p]);
        self.set_space_factor(1000);
        Ok(())
    }

    /// §1127
    pub(crate) fn align_error(&mut self) -> Result<(), Jump> {
        if self.align_state().abs() > 2 {
            // §1128: express consternation over the fact that no alignment
            // is in progress.
            self.print_err(b"Misplaced ");
            self.print_cmd_chr(self.cur_cmd, self.cur_chr);
            if self.cur_tok == TAB_TOKEN + i32::from(b'&') {
                self.help(&[
                    b"I can't figure out why you would want to use a tab mark",
                    b"here. If you just want an ampersand, the remedy is",
                    b"simple: Just type `I\\&' now. But if some right brace",
                    b"up above has ended a previous alignment prematurely,",
                    b"you're probably due for more error messages, and you",
                    b"might try typing `S' now just to see what is salvageable.",
                ]);
            } else {
                self.help(&[
                    b"I can't figure out why you would want to use a tab mark",
                    b"or \\cr or \\span just now. If something like a right brace",
                    b"up above has ended a previous alignment prematurely,",
                    b"you're probably due for more error messages, and you",
                    b"might try typing `S' now just to see what is salvageable.",
                ]);
            }
            return self.error();
        }
        self.back_input()?;
        if self.align_state() < 0 {
            self.print_err(b"Missing { inserted");
            self.set_align_state(self.align_state() + 1);
            self.cur_tok = LEFT_BRACE_TOKEN + i32::from(b'{');
        } else {
            self.print_err(b"Missing } inserted");
            self.set_align_state(self.align_state() - 1);
            self.cur_tok = RIGHT_BRACE_TOKEN + i32::from(b'}');
        }
        self.help(&[
            b"I've put in what seems to be necessary to fix",
            b"the current column of the current alignment.",
            b"Try to go on, since this might almost work.",
        ]);
        self.ins_error()
    }

    /// §1129
    pub(crate) fn no_align_error(&mut self) -> Result<(), Jump> {
        self.print_err(b"Misplaced ");
        self.print_esc(b"noalign");
        self.help(&[
            b"I expect to see \\noalign only after the \\cr of",
            b"an alignment. Proceed, and I'll ignore this case.",
        ]);
        self.error()
    }

    /// §1129
    pub(crate) fn omit_error(&mut self) -> Result<(), Jump> {
        self.print_err(b"Misplaced ");
        self.print_esc(b"omit");
        self.help(&[
            b"I expect to see \\omit only after tab marks or the \\cr of",
            b"an alignment. Proceed, and I'll ignore this case.",
        ]);
        self.error()
    }

    /// §1131
    pub(crate) fn do_endv(&mut self) -> Result<(), Jump> {
        self.base_ptr = self.input_ptr;
        self.input_stack[self.base_ptr] = self.cur_input.clone();
        loop {
            let s = self.input_stack[self.base_ptr].clone();
            if s.index != V_TEMPLATE && s.loc == NULL && s.state == TOKEN_LIST {
                self.base_ptr -= 1;
            } else {
                break;
            }
        }
        let s = self.input_stack[self.base_ptr].clone();
        if s.index != V_TEMPLATE || s.loc != NULL || s.state != TOKEN_LIST {
            return self.fatal_error(b"(interwoven alignment preambles are not allowed)");
        }
        if self.cur_group() == ALIGN_GROUP {
            self.end_graf()?;
            if self.fin_col()? {
                self.fin_row()?;
            }
            Ok(())
        } else {
            self.off_save()
        }
    }

    /// §1135
    pub(crate) fn cs_error(&mut self) -> Result<(), Jump> {
        self.print_err(b"Extra ");
        if self.cur_chr == 10 {
            self.print_esc(b"endmubyte");
            self.help(&[b"I'm ignoring this, since I wasn't doing a \\mubyte."]);
        } else {
            self.print_esc(b"endcsname");
            self.help(&[b"I'm ignoring this, since I wasn't doing a \\csname."]);
        }
        self.error()
    }

    /// §1049
    pub(crate) fn you_cant(&mut self) {
        self.print_err(b"You can't use `");
        self.print_cmd_chr(self.cur_cmd, self.cur_chr);
        self.print_in_mode(self.mode());
    }

    /// §1050
    pub(crate) fn report_illegal_case(&mut self) -> Result<(), Jump> {
        self.you_cant();
        self.help(&[
            b"Sorry, but I'm not programmed to handle this case;",
            b"I'll just pretend that you didn't ask for it.",
            b"If you're in the wrong mode, you might be able to",
            b"return to the right one by typing `I}' or `I$' or `I\\par'.",
        ]);
        self.error()
    }

    /// §1051
    pub(crate) fn privileged(&mut self) -> Result<bool, Jump> {
        if self.mode() > 0 {
            Ok(true)
        } else {
            self.report_illegal_case()?;
            Ok(false)
        }
    }

    /// §1047
    pub(crate) fn insert_dollar_sign(&mut self) -> Result<(), Jump> {
        self.back_input()?;
        self.cur_tok = MATH_SHIFT_TOKEN + i32::from(b'$');
        self.print_err(b"Missing $ inserted");
        self.help(&[
            b"I've inserted a begin-math/end-math symbol since I think",
            b"you left one out. Proceed, with fingers crossed.",
        ]);
        self.ins_error()
    }

    /// §1054: do this when \end or \dump occurs.
    pub(crate) fn its_all_over(&mut self) -> Result<bool, Jump> {
        if self.privileged()? {
            if self.page_list_len() == 0 && self.list_is_empty() && self.dead_cycles() == 0 {
                return Ok(true);
            }
            self.back_input()?; // we will try to end again after ejecting residual material
            let b = BoxNode {
                width: self.dimen_par(HSIZE_CODE),
                ..BoxNode::default()
            };
            self.tail_append(Node::Box(b.share()));
            self.tail_append(new_glue(FILL_GLUE));
            self.tail_append(Node::Penalty(-0o10000000000));
            self.build_page()?; // append \hbox to \hsize{}\vfill\penalty-'10000000000
        }
        Ok(false)
    }
}

/// pdfTeX's `unpackage` drops the margin kerns of what it appends; its
/// loop steps over the node after each one it drops, so of two margin
/// kerns in a row the second stays.
fn without_margin_kerns(mut list: Vec<Node>) -> Vec<Node> {
    if list.iter().any(|n| matches!(n, Node::MarginKern { .. })) {
        let mut skip = false;
        list.retain(|n| {
            let drop = !skip && matches!(n, Node::MarginKern { .. });
            skip = drop;
            !drop
        });
    }
    list
}
