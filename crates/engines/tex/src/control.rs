//! One command (§1029–§1045 `main_control`'s big switch, §1208–§1298
//! `prefixed_command` and the mode-independent commands): what a node
//! does with the first token of its input.

use std::rc::Rc;

use phitex_defuse::{Value, ValueId};
use phitex_typeset::glue::Glue;

use crate::fields::Par;
use crate::run::{R, Run};
use crate::runstate::{Loc, Native};
use crate::val::{
    CAT_CODE_BASE, DEL_CODE_BASE, MATH_CODE_BASE, Macro, Mean, OUTPUT_ROUTINE_LOC, SF_CODE_BASE,
    TOKS_BASE, Tok, cmd, glue_of, int_of, list_of, str_of, toks_value,
};

const BOX_FLAG: i64 = 0o10000000000;
pub(crate) const SHIP_OUT_FLAG: i64 = BOX_FLAG + 512;
pub(crate) const LEADER_FLAG: i64 = BOX_FLAG + 513;

/// `PHITEX_HEARTBEAT_FROM`: the command count the per-command trace starts at.
static HEARTBEAT_FROM: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

impl Run<'_, '_> {
    /// The node's command: expand once, or do one command (§1030).
    pub(crate) fn command(&mut self) -> R<()> {
        self.heartbeat();
        self.no_pull = true;
        let got = self.get_next();
        self.no_pull = false;
        if !got? {
            return Ok(());
        }
        if self.cur.cmd > cmd::MAX_COMMAND {
            if self.cur.cmd >= cmd::END_TEMPLATE && self.cur.cmd != cmd::DONT_EXPAND {
                return Ok(());
            }
            return self.expand();
        }
        self.dispatch()
    }

    /// With `PHITEX_HEARTBEAT=1`, every 100,000 commands on this thread:
    /// elapsed seconds, commands, the innermost file and line, and the
    /// run's sizes, on stderr (a diagnostic for long serial runs).
    fn heartbeat(&self) {
        use std::cell::Cell;
        use std::sync::OnceLock;
        use std::time::Instant;
        static ON: OnceLock<bool> = OnceLock::new();
        static EXIT: OnceLock<u64> = OnceLock::new();
        thread_local! {
            static N: Cell<u64> = const { Cell::new(0) };
            static T0: Cell<Option<Instant>> = const { Cell::new(None) };
        }
        if !*ON.get_or_init(|| std::env::var("PHITEX_HEARTBEAT").is_ok_and(|v| v == "1")) {
            return;
        }
        let n = N.with(|c| {
            c.set(c.get() + 1);
            c.get()
        });
        let t0 = T0.with(|c| {
            let t = c.get().unwrap_or_else(Instant::now);
            c.set(Some(t));
            t
        });
        let from = *HEARTBEAT_FROM.get_or_init(|| {
            std::env::var("PHITEX_HEARTBEAT_FROM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(u64::MAX)
        });
        let trace = n >= from && n < from.saturating_add(150_000);
        if !n.is_multiple_of(100_000) && !trace {
            return;
        }
        // `PHITEX_HEARTBEAT_EXIT`: the process ends at this many commands
        // (a profile of a run's first commands)
        let exit = *EXIT.get_or_init(|| {
            std::env::var("PHITEX_HEARTBEAT_EXIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(u64::MAX)
        });
        if n >= exit {
            std::process::exit(0);
        }
        let (file, line) = self
            .frames
            .iter()
            .find_map(|f| match f {
                crate::run::Frame::File(f) => Some(f.where_(self.v)),
                _ => None,
            })
            .unwrap_or_default();
        let mut last = String::new();
        crate::print::print_cs(&mut last, self.cur.tok, i64::from(b'\\'), &mut |c| {
            if char::from_u32(c).is_some_and(char::is_alphabetic) {
                11
            } else {
                12
            }
        });
        eprintln!(
            "hb{} {:.1}s cmds {n} {file}:{line} last {last} cmd {} mode {} state {:?} nest {} frames {}",
            if trace { "t" } else { "" },
            t0.elapsed().as_secs_f64(),
            self.cur.cmd,
            self.nest.last().map_or(0, |l| l.mode),
            self.st.sig(),
            self.nest.len(),
            self.frames.len()
        );
    }

    /// `reswitch` (§1045): the case for `cur` in the current mode.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn dispatch(&mut self) -> R<()> {
        let md = self.mode()?;
        if crate::etex::command(self, &md)? {
            return Ok(());
        }
        let am = md.m.abs();
        let (v, h) = (am == 1, am == 102);
        let c = self.cur.cmd;
        if am == 203 {
            if self.math_command(&md)? {
                return Ok(());
            }
        } else if crate::math::non_math(c) {
            return self.insert_dollar_sign();
        }
        match c {
            cmd::RELAX => {}
            cmd::IGNORE => self.do_endv(&md)?,
            cmd::NO_ALIGN if self.cur.chr == 1 => self.align_peek(&md)?,
            cmd::SPACER => {
                if h {
                    self.char_run(&md, false)?;
                }
            }
            cmd::LETTER
            | cmd::OTHER_CHAR
            | cmd::CHAR_GIVEN
            | cmd::CHAR_NUM
            | cmd::NO_BOUNDARY
            | cmd::EX_SPACE => {
                if v {
                    self.back_cur();
                    self.new_graf(&md, true)?;
                } else if h {
                    self.char_run(&md, false)?;
                }
            }
            cmd::IGNORE_SPACES => {
                self.skip_blanks_x()?;
                return self.dispatch();
            }
            cmd::STOP => {
                if v {
                    self.its_all_over(&md)?;
                } else if h {
                    self.head_for_vmode(&md);
                }
            }
            cmd::HRULE | cmd::VRULE => {
                if (c == cmd::HRULE) == v {
                    let (w, ht, d) = self.scan_rule_spec(c == cmd::VRULE)?;
                    let node = self.v.list(vec![
                        self.v.int(crate::nodes::T_RULE),
                        self.v.int(i64::from(w)),
                        self.v.int(i64::from(ht)),
                        self.v.int(i64::from(d)),
                    ]);
                    self.append(md.lid, node);
                    if v {
                        self.set_prev_depth(&md, crate::build::IGNORE_DEPTH);
                    } else {
                        self.set_space_factor(&md, 1000);
                    }
                } else if v {
                    self.back_cur();
                    self.new_graf(&md, true)?;
                } else if h {
                    self.head_for_vmode(&md);
                }
            }
            cmd::VSKIP | cmd::HSKIP | cmd::MSKIP => {
                if (c == cmd::VSKIP) == v {
                    self.append_glue(&md)?;
                } else if v {
                    self.back_cur();
                    self.new_graf(&md, true)?;
                } else if h {
                    self.head_for_vmode(&md);
                }
            }
            cmd::KERN | cmd::MKERN => {
                // §1061
                let s = self.cur.chr;
                let w = self.scan_dimen(s == 99, false, None)?;
                let k = if s == 99 {
                    phitex_typeset::node::KernKind::Mu
                } else {
                    phitex_typeset::node::KernKind::Explicit
                };
                let node = crate::nodes::kern_node(self.v, w, k);
                self.append(md.lid, node);
            }
            cmd::LEFT_BRACE => self.open_group(&md, crate::val::group::SIMPLE, vec![])?,
            cmd::BEGIN_GROUP => self.open_group(&md, crate::val::group::SEMI_SIMPLE, vec![])?,
            cmd::END_GROUP => {
                if md.group == crate::val::group::SEMI_SIMPLE {
                    self.close_simple(&md)?;
                }
                // otherwise `off_save` (§1064): an error
            }
            cmd::RIGHT_BRACE => self.handle_right_brace(&md)?,
            cmd::HMOVE | cmd::VMOVE => {
                if (c == cmd::HMOVE) == v {
                    let t = self.cur.chr;
                    let d = self.scan_normal_dimen()?;
                    self.scan_box(&md, if t == 0 { i64::from(d) } else { -i64::from(d) })?;
                }
            }
            cmd::LEADER_SHIP => {
                let ctx = LEADER_FLAG - 100 + i64::from(self.cur.chr);
                self.scan_box(&md, ctx)?;
            }
            cmd::MAKE_BOX => self.begin_box(&md, 0)?,
            cmd::START_PAR => {
                if v {
                    let ind = self.cur.chr > 0;
                    self.new_graf(&md, ind)?;
                } else if self.cur.chr > 0 {
                    // §1093
                    let w = self.ipar(Par::parindent)?;
                    let b = crate::build::null_box(self.v, w);
                    self.append(md.lid, b);
                    if h {
                        self.set_space_factor(&md, 1000);
                    }
                }
            }
            cmd::PAR_END => {
                // §1094
                if v {
                    self.normal_paragraph()?;
                    if md.m > 0 {
                        self.build_page_outer()?;
                    }
                } else if h {
                    self.end_graf(&md)?;
                    self.build_page_outer()?;
                }
            }
            cmd::UN_HBOX => {
                if v {
                    self.back_cur();
                    self.new_graf(&md, true)?;
                } else {
                    self.unpackage(&md)?;
                }
            }
            cmd::UN_VBOX => {
                if v {
                    self.unpackage(&md)?;
                } else if h {
                    self.head_for_vmode(&md);
                }
            }
            cmd::HALIGN | cmd::VALIGN => {
                if c == cmd::VALIGN && v {
                    self.back_cur();
                    self.new_graf(&md, true)?;
                } else if c == cmd::HALIGN && h {
                    self.head_for_vmode(&md);
                } else if c == cmd::HALIGN && v {
                    self.init_align(&md)?;
                } else {
                    // REDUCE-GAP: \valign and alignments in displays
                    self.log("error", "this alignment is not supported")?;
                }
            }
            cmd::INSERT => self.begin_insert_or_adjust(&md, false)?,
            cmd::VADJUST => {
                if !v {
                    self.begin_insert_or_adjust(&md, true)?;
                }
            }
            cmd::MARK => {
                // §1101
                let (_, body) = self.scan_toks(false, true)?;
                let node = self.v.list(vec![
                    self.v.int(crate::nodes::T_MARK),
                    toks_value(self.v, &body),
                ]);
                self.append(md.lid, node);
            }
            cmd::BREAK_PENALTY => {
                // §1103
                let n = self.scan_int()?;
                let node = crate::nodes::penalty_node(self.v, n);
                self.append(md.lid, node);
                if md.m == 1 {
                    self.build_page_outer()?;
                }
            }
            cmd::REMOVE_ITEM => self.delete_last(&md)?,
            cmd::ITAL_CORR => {
                if h {
                    self.append_italic_correction(&md)?;
                } else if am == 203 {
                    let node =
                        crate::nodes::kern_node(self.v, 0, phitex_typeset::node::KernKind::Normal);
                    self.append(md.lid, node);
                }
            }
            cmd::DISCRETIONARY => {
                if v {
                    self.back_cur();
                    self.new_graf(&md, true)?;
                } else {
                    self.append_discretionary(&md)?;
                }
            }
            cmd::ACCENT => {
                if v {
                    self.back_cur();
                    self.new_graf(&md, true)?;
                } else if h {
                    self.make_accent(&md)?;
                }
            }
            cmd::MATH_SHIFT => {
                if v {
                    self.back_cur();
                    self.new_graf(&md, true)?;
                } else if h {
                    self.init_math(&md)?;
                }
            }
            cmd::TOKS_REGISTER..=cmd::SET_INTERACTION => self.prefixed_command()?,
            cmd::AFTER_ASSIGNMENT => {
                // §1268
                self.get_token()?;
                let t = self.cur.tok.encode(self.v);
                self.set_cell(crate::fields::cell::AFTERTOKEN, t, true);
            }
            cmd::AFTER_GROUP => {
                // §1271
                self.get_token()?;
                let t = self.cur.tok;
                // the innermost group's level keeps the tokens; at the
                // bottom level they are dropped (§1271 `cur_level>level_one`)
                if let Some(l) = self.nest.iter_mut().rev().find(|l| l.group != 0) {
                    l.ag.push(t);
                }
            }
            cmd::IN_STREAM => self.open_or_close_in()?,
            cmd::MESSAGE => {
                // §1279
                let ch = self.cur.chr;
                let (_, body) = self.scan_toks(false, true)?;
                let mut s = String::new();
                self.show_toks_into(&mut s, &body)?;
                self.log(if ch == 0 { "msg" } else { "errmsg" }, &s)?;
            }
            cmd::CASE_SHIFT => {
                // §1288
                let b = self.cur.chr;
                let (_, body) = self.scan_toks(false, false)?;
                let table = if b == crate::val::LC_CODE_BASE {
                    "lc"
                } else {
                    "uc"
                };
                let mut out = Vec::with_capacity(body.len());
                for t in body {
                    match t.as_cs() {
                        None => {
                            let ch = t.chr() & 0xFF;
                            let e = self.int(&format!("{table}:{ch}"))?;
                            if e != 0 {
                                out.push(Tok::ch(
                                    t.cat().unwrap_or(0),
                                    u32::try_from(e).unwrap_or(0),
                                ));
                            } else {
                                out.push(t);
                            }
                        }
                        Some(_) => out.push(t),
                    }
                }
                self.push_toks(out);
            }
            cmd::XRAY => self.show_whatever()?,
            cmd::EXTENSION => self.do_extension(&md)?,
            _ => {
                // forbidden cases, alignment errors, math in non-math
                // modes (§1046–§1048): errors
                self.log("error", "illegal command")?;
            }
        }
        Ok(())
    }

    /// `prefixed_command` (§1211–§1269).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn prefixed_command(&mut self) -> R<()> {
        let mut a: i32 = 0;
        while self.cur.cmd == cmd::PREFIX {
            if (a / self.cur.chr) % 2 == 0 {
                a += self.cur.chr;
            }
            self.skip_blanks_relax_x()?;
            if self.cur.cmd <= cmd::MAX_NON_PREFIXED {
                // §1212: "You can't use a prefix with"
                self.back_cur();
                return Ok(());
            }
        }
        let protected = crate::etex::take_protected(&mut a);
        // §1214
        let gd = self.ipar(Par::globaldefs)?;
        if gd != 0 {
            if gd < 0 {
                if a >= 4 {
                    a -= 4;
                }
            } else if a < 4 {
                a += 4;
            }
        }
        let global = a >= 4;
        let v = self.v;
        if crate::etex::prefixed(self, global)? {
            return self.after_assignment();
        }
        match self.cur.cmd {
            cmd::SET_FONT => {
                // §1217
                let f = v.int(i64::from(self.cur.chr));
                self.set_cell(crate::fields::cell::FONT, f, global);
            }
            cmd::DEF => {
                // §1218
                let ch = self.cur.chr;
                let mut global = global;
                if ch % 2 == 1 && !global && gd >= 0 {
                    global = true;
                }
                let e = ch >= 2;
                let p = self.get_r_token()?;
                let (params, body) = self.scan_toks(true, e)?;
                let m = Mean::Macro(Rc::new(Macro {
                    flags: u8::try_from(a % 4).unwrap_or(0)
                        | if protected { crate::etex::PROTECTED } else { 0 },
                    params,
                    body: body.into(),
                }));
                self.define_cs(&p, &m, global);
            }
            cmd::LET => {
                // §1221
                let n = self.cur.chr;
                let p = self.get_r_token()?;
                if n == 0 {
                    loop {
                        self.get_token()?;
                        if self.cur.cmd != cmd::SPACER {
                            break;
                        }
                    }
                    if self.is_other(b'=') {
                        self.get_token()?;
                        if self.cur.cmd == cmd::SPACER {
                            self.get_token()?;
                        }
                    }
                } else {
                    self.get_token()?;
                    let q = self.cur.tok;
                    self.get_token()?;
                    let t = self.cur.tok;
                    self.back_input(t);
                    self.back_input(q);
                }
                let m = self.cur.mean.clone();
                self.define_cs(&p, &m, global);
            }
            cmd::SHORTHAND_DEF => {
                // §1224
                let n = self.cur.chr;
                let p = self.get_r_token()?;
                self.define_cs(&p, &Mean::Prim("relax", cmd::RELAX, 256), global);
                self.scan_optional_equals()?;
                let m = match n {
                    0 => Mean::CharDef(i64::from(self.scan_char_num()?)),
                    1 => Mean::MathCharDef(i64::from(self.scan_fifteen_bit_int()?)),
                    _ => {
                        let r = self.scan_eight_bit_int()?;
                        match n {
                            2 => Mean::Reg(cmd::ASSIGN_INT, crate::val::COUNT_BASE + r),
                            3 => Mean::Reg(cmd::ASSIGN_DIMEN, crate::val::SCALED_BASE + r),
                            4 => Mean::Reg(cmd::ASSIGN_GLUE, crate::val::SKIP_BASE + r),
                            5 => Mean::Reg(cmd::ASSIGN_MU_GLUE, crate::val::MU_SKIP_BASE + r),
                            _ => Mean::Reg(cmd::ASSIGN_TOKS, TOKS_BASE + r),
                        }
                    }
                };
                self.define_cs(&p, &m, global);
            }
            cmd::READ_TO_CS => {
                // §1225
                let n = self.scan_int()?;
                let _ = self.scan_keyword("to")?;
                let p = self.get_r_token()?;
                let body = self.read_toks(n)?;
                let m = Mean::Macro(Rc::new(Macro {
                    flags: 0,
                    params: Vec::new(),
                    body: body.into(),
                }));
                self.define_cs(&p, &m, global);
            }
            cmd::TOKS_REGISTER | cmd::ASSIGN_TOKS => {
                // §1226
                let loc = if self.cur.cmd == cmd::TOKS_REGISTER {
                    TOKS_BASE + self.scan_eight_bit_int()?
                } else {
                    self.cur.chr
                };
                let field = self.quantity(cmd::ASSIGN_TOKS, loc);
                self.scan_optional_equals()?;
                self.skip_blanks_relax_x()?;
                if self.cur.cmd != cmd::LEFT_BRACE {
                    // §1227
                    let mut src = None;
                    if self.cur.cmd == cmd::TOKS_REGISTER {
                        let r = self.scan_eight_bit_int()?;
                        src = Some(TOKS_BASE + r);
                    } else if self.cur.cmd == cmd::ASSIGN_TOKS {
                        src = Some(self.cur.chr);
                    }
                    if let Some(s) = src {
                        let x = self.qget(cmd::ASSIGN_TOKS, s)?;
                        let val = x.unwrap_or_else(|| v.list(Vec::new()));
                        self.set_f(field, val, global);
                        return self.after_assignment();
                    }
                }
                self.back_cur();
                let (_, mut body) = self.scan_toks(false, false)?;
                if !body.is_empty() && loc == OUTPUT_ROUTINE_LOC {
                    body.insert(0, Tok::ch(1, u32::from(b'{')));
                    body.push(Tok::ch(2, u32::from(b'}')));
                }
                let val = toks_value(v, &body);
                self.set_f(field, val, global);
            }
            cmd::ASSIGN_INT => {
                let l = self.quantity_loc(cmd::ASSIGN_INT, self.cur.chr);
                self.scan_optional_equals()?;
                let n = self.scan_int()?;
                self.set_loc(l, Native::Int(n), global);
            }
            cmd::ASSIGN_DIMEN => {
                let l = self.quantity_loc(cmd::ASSIGN_DIMEN, self.cur.chr);
                self.scan_optional_equals()?;
                let n = self.scan_normal_dimen()?;
                self.set_loc(l, Native::Int(n), global);
            }
            cmd::ASSIGN_GLUE | cmd::ASSIGN_MU_GLUE => {
                let n = self.cur.cmd;
                let l = self.quantity_loc(n, self.cur.chr);
                self.scan_optional_equals()?;
                let g = self.scan_glue(if n == cmd::ASSIGN_MU_GLUE { 3 } else { 2 })?;
                self.set_loc(l, Native::Glue(g), global);
            }
            cmd::DEF_CODE => {
                // §1232
                let base = self.cur.chr;
                let n: i32 = match base {
                    CAT_CODE_BASE => 15,
                    MATH_CODE_BASE => 0o100000,
                    SF_CODE_BASE => 0o77777,
                    DEL_CODE_BASE => 0o77777777,
                    _ => 255,
                };
                let ch = self.scan_char_num()?;
                self.scan_optional_equals()?;
                let mut val = self.scan_int()?;
                if (val < 0 && base < DEL_CODE_BASE) || val > n {
                    // "Invalid code"
                    val = 0;
                }
                let (name, table) = match base {
                    CAT_CODE_BASE => ("cat", None),
                    crate::val::LC_CODE_BASE => ("lc", Some(0)),
                    crate::val::UC_CODE_BASE => ("uc", Some(1)),
                    SF_CODE_BASE => ("sf", Some(2)),
                    MATH_CODE_BASE => ("mc", Some(3)),
                    _ => ("dc", Some(4)),
                };
                let c = u32::try_from(ch).unwrap_or(u32::MAX);
                let fixed = match table {
                    None => (c < 256).then(|| crate::fields::CAT0 + c as usize),
                    Some(t) => crate::fields::code_slot(t, c),
                };
                let l = if let Some(s) = fixed {
                    Loc::try_from(s).unwrap_or(0)
                } else {
                    let f = self.v.field(&format!("{name}:{ch}"));
                    self.loc_of(f)
                };
                self.set_loc(l, Native::Int(val), global);
            }
            cmd::DEF_FAMILY => {
                // §1234
                let size = self.cur.chr - crate::val::MATH_FONT_BASE;
                let n = self.scan_four_bit_int()?;
                self.scan_optional_equals()?;
                let f = self.scan_font_ident()?;
                self.set(&format!("fam:{size}:{n}"), v.int(f), global);
            }
            cmd::REGISTER | cmd::ADVANCE | cmd::MULTIPLY | cmd::DIVIDE => {
                self.do_register_command(global)?;
            }
            cmd::SET_BOX => {
                // §1241
                let n = self.scan_eight_bit_int()?;
                self.scan_optional_equals()?;
                let md = self.mode()?;
                let ctx = BOX_FLAG + i64::from(n) + if global { 256 } else { 0 };
                self.scan_box(&md, ctx)?;
            }
            cmd::SET_AUX => {
                // §1243
                let md = self.mode()?;
                let c = self.cur.chr;
                if c == md.m.abs() {
                    self.scan_optional_equals()?;
                    if c == 1 {
                        let d = self.scan_normal_dimen()?;
                        self.set_prev_depth(&md, d);
                    } else {
                        let n = self.scan_int()?;
                        if n > 0 && n <= 32767 {
                            self.set_space_factor(&md, n);
                        }
                    }
                }
            }
            cmd::SET_PREV_GRAF => {
                // §1244
                let md = self.mode()?;
                self.scan_optional_equals()?;
                let n = self.scan_int()?;
                if n >= 0 {
                    self.set(&format!("pg:{}", md.vlid), v.int(i64::from(n)), true);
                }
            }
            cmd::SET_PAGE_DIMEN => {
                // §1245
                // REDUCE-GAP: resumable fold: the page so far is not a
                // binding while the document runs; the value is dropped.
                self.scan_optional_equals()?;
                let _ = self.scan_normal_dimen()?;
            }
            cmd::SET_PAGE_INT => {
                // §1246
                let c = self.cur.chr;
                self.scan_optional_equals()?;
                let n = self.scan_int()?;
                self.set(
                    if c == 0 { "deadcycles" } else { "inspen" },
                    v.int(i64::from(n)),
                    true,
                );
            }
            cmd::SET_BOX_DIMEN => {
                // §1247
                let c = self.cur.chr;
                let b = self.scan_eight_bit_int()?;
                self.scan_optional_equals()?;
                let d = self.scan_normal_dimen()?;
                if let Some(bx) = self.box_reg(b)? {
                    let nb = crate::nodes::box_with_dim(v, bx, c, d);
                    self.modify_box_reg(b, Some(nb))?;
                }
            }
            cmd::SET_SHAPE => {
                // §1248
                self.scan_optional_equals()?;
                let n = self.scan_int()?;
                let mut xs = Vec::new();
                for _ in 0..n.max(0) {
                    let i = self.scan_normal_dimen()?;
                    let w = self.scan_normal_dimen()?;
                    xs.push(v.int(i64::from(i)));
                    xs.push(v.int(i64::from(w)));
                }
                self.set_cell(crate::fields::cell::PARSHAPE, v.list(xs), global);
            }
            cmd::HYPH_DATA => {
                // §1252
                if self.cur.chr == 1 {
                    self.new_patterns()?;
                } else {
                    self.new_hyph_exceptions()?;
                }
            }
            cmd::ASSIGN_FONT_DIMEN => {
                // §1253
                let fd = self.find_font_dimen(true)?;
                self.scan_optional_equals()?;
                let d = self.scan_normal_dimen()?;
                if let Some((f, k)) = fd {
                    self.set(&format!("fd:{f}:{k}"), v.int(i64::from(d)), true);
                }
            }
            cmd::ASSIGN_FONT_INT => {
                let n = self.cur.chr;
                let f = self.scan_font_ident()?;
                self.scan_optional_equals()?;
                let x = self.scan_int()?;
                self.set(
                    &format!("{}:{f}", if n == 0 { "hc" } else { "sc" }),
                    v.int(i64::from(x)),
                    true,
                );
            }
            cmd::DEF_FONT => self.new_font(global)?,
            cmd::SET_INTERACTION => {
                // §1265
                self.log("println", "")?;
            }
            _ => {}
        }
        self.after_assignment()
    }

    /// §1269: the token saved by `\afterassignment`, if any.
    pub(crate) fn after_assignment(&mut self) -> R<()> {
        if let Some(t) = self.get_loc(crate::fields::cell::AFTERTOKEN)?
            && t != ValueId::UNIT
        {
            let tok = Tok::decode(self.v, t);
            self.back_input(tok);
            self.set_cell(crate::fields::cell::AFTERTOKEN, ValueId::UNIT, true);
        }
        Ok(())
    }

    /// `get_r_token` (§1215).
    fn get_r_token(&mut self) -> R<Tok> {
        loop {
            self.get_token()?;
            if self.cur.tok != crate::val::space_tok() {
                break;
            }
        }
        if !self.cur.is_cs() {
            // "Missing control sequence inserted": \inaccessible
            self.back_cur();
            return Ok(Tok::cs("inaccessible"));
        }
        Ok(self.cur.tok)
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub(crate) fn define_cs(&mut self, t: &Tok, m: &Mean, global: bool) {
        self.set_mean(*t, m.clone(), global);
    }

    /// `do_register_command` (§1236–§1240).
    #[allow(clippy::too_many_lines)]
    fn do_register_command(&mut self, global: bool) -> R<()> {
        let q = self.cur.cmd;
        let v = self.v;
        // §1237
        let (field, p): (phitex_defuse::Field, u8) = if q == cmd::REGISTER {
            let p = u8::try_from(self.cur.chr).unwrap_or(0);
            let n = self.scan_eight_bit_int()?;
            (self.reg_field(usize::from(p.min(3)), n), p)
        } else {
            self.get_x_token()?;
            if self.cur.cmd >= cmd::ASSIGN_INT && self.cur.cmd <= cmd::ASSIGN_MU_GLUE {
                (
                    self.quantity(self.cur.cmd, self.cur.chr),
                    self.cur.cmd - cmd::ASSIGN_INT,
                )
            } else if self.cur.cmd == cmd::REGISTER {
                let p = u8::try_from(self.cur.chr).unwrap_or(0);
                let n = self.scan_eight_bit_int()?;
                (self.reg_field(usize::from(p.min(3)), n), p)
            } else {
                // "You can't use x after \advance"
                return Ok(());
            }
        };
        if q == cmd::REGISTER {
            self.scan_optional_equals()?;
        } else {
            let _ = self.scan_keyword("by")?;
        }
        let old = self.read_f(field)?;
        if p < 2 {
            let cur = i64::from(crate::val::i32_of(v, old));
            let r: Option<i32> = if q < cmd::MULTIPLY {
                let x = if p == 0 {
                    self.scan_int()?
                } else {
                    self.scan_normal_dimen()?
                };
                if q == cmd::ADVANCE {
                    let s = cur + i64::from(x);
                    // web2c: integer overflow wraps silently
                    // the low 32 bits
                    #[allow(clippy::cast_possible_truncation)]
                    let w = s as i32;
                    Some(w)
                } else {
                    Some(x)
                }
            } else {
                let x = self.scan_int()?;
                let cur = i32::try_from(cur).unwrap_or(0);
                if q == cmd::MULTIPLY {
                    if p == 0 {
                        phitex_typeset::arith::mult_integers(cur, x)
                    } else {
                        phitex_typeset::arith::nx_plus_y(cur, x, 0)
                    }
                } else {
                    let d = phitex_typeset::arith::x_over_n(cur, x);
                    (!d.overflow).then_some(d.value)
                }
            };
            let Some(r) = r else {
                // "Arithmetic overflow"
                return Ok(());
            };
            let l = self.loc_of(field);
            self.set_loc(l, Native::Int(r), global);
        } else {
            let s = glue_of(v, old);
            let r: Option<Glue> = if q < cmd::MULTIPLY {
                let g = self.scan_glue(p)?;
                if q == cmd::ADVANCE {
                    Some(add_glue(&g, &s))
                } else {
                    Some(g)
                }
            } else {
                let x = self.scan_int()?;
                if q == cmd::MULTIPLY {
                    let m = |a: i32| phitex_typeset::arith::nx_plus_y(a, x, 0);
                    match (m(s.width), m(s.stretch), m(s.shrink)) {
                        (Some(w), Some(st), Some(sh)) => Some(Glue {
                            width: w,
                            stretch: st,
                            shrink: sh,
                            ..s
                        }),
                        _ => None,
                    }
                } else {
                    let d = |a: i32| phitex_typeset::arith::x_over_n(a, x);
                    let (w, st, sh) = (d(s.width), d(s.stretch), d(s.shrink));
                    (!(w.overflow || st.overflow || sh.overflow)).then_some(Glue {
                        width: w.value,
                        stretch: st.value,
                        shrink: sh.value,
                        ..s
                    })
                }
            };
            let Some(r) = r else { return Ok(()) };
            let l = self.loc_of(field);
            self.set_loc(l, Native::Glue(r), global);
        }
        Ok(())
    }

    /// `new_font` (§1257–§1260).
    fn new_font(&mut self, global: bool) -> R<()> {
        let u = self.get_r_token()?;
        let t: String = match u.as_cs() {
            Some(n) if n.is_empty() => "FONT".to_owned(),
            Some(n) => n.to_string(),
            None => format!("FONT{}", char::from_u32(u.chr()).unwrap_or('?')),
        };
        self.define_cs(&u, &Mean::Font(0), global);
        self.scan_optional_equals()?;
        let name = self.scan_file_name()?;
        // §1258
        let s: i32 = if self.scan_keyword("at")? {
            let s = self.scan_normal_dimen()?;
            if s <= 0 || s >= 0o1000000000 {
                10 * 65536
            } else {
                s
            }
        } else if self.scan_keyword("scaled")? {
            let n = self.scan_int()?;
            if n <= 0 || n > 32768 { -1000 } else { -n }
        } else {
            -1000
        };
        // §1260
        let count = self.cint(crate::fields::cell::FONTCOUNT)?;
        let mut found = None;
        for f in 1..=count {
            let info = self.get(&format!("fontinfo:{f}"))?;
            if let Some(i) = crate::fonts::info(self.v, info)
                && i.name == name
            {
                let same = if s > 0 {
                    s == i.size
                } else {
                    i.size == phitex_typeset::arith::xn_over_d(i.dsize, -s, 1000).value
                };
                if same {
                    found = Some(f);
                    break;
                }
            }
        }
        let f = match found {
            Some(f) => f,
            None => self.read_font_info(&name, s)?,
        };
        let v = self.v;
        self.define_cs(&u, &Mean::Font(f), global);
        // §1257: the font identifier
        let id = Tok::cs(&format!("\u{10FFFE}F{f}\u{2}{t}"));
        self.define_cs(&id, &Mean::Font(f), true);
        self.set(&format!("fontid:{f}"), v.str(&t), true);
        Ok(())
    }

    /// `read_font_info` (§560): a new font, or the null font if the TFM
    /// can't be had.
    pub(crate) fn read_font_info(&mut self, name: &str, s: i32) -> R<i64> {
        let key = format!("{name}.tfm");
        let Some(file) = self.file(&key)? else {
            self.log("error", "Font not loadable")?;
            return Ok(0);
        };
        let v = self.v;
        let Some(l) = crate::fonts::load(v, file, s) else {
            self.log("error", "Font not loadable")?;
            return Ok(0);
        };
        let f = self.cint(crate::fields::cell::FONTCOUNT)? + 1;
        self.set_cell(crate::fields::cell::FONTCOUNT, v.int(f), true);
        let info = v.list(vec![
            v.str(name),
            v.int(i64::from(l.at.size)),
            v.int(i64::from(l.at.design_size)),
            file,
            v.int(i64::from(s)),
        ]);
        self.set(&format!("fontinfo:{f}"), info, true);
        let params = l.at.params.clone();
        for (k, p) in params.iter().enumerate() {
            self.set(&format!("fd:{f}:{}", k + 1), v.int(i64::from(*p)), true);
        }
        self.set(
            &format!("fdn:{f}"),
            v.int(i64::try_from(params.len()).unwrap_or(0)),
            true,
        );
        let hc = i64::from(self.ipar(Par::defaulthyphenchar)?);
        let sc = i64::from(self.ipar(Par::defaultskewchar)?);
        self.set(&format!("hc:{f}"), v.int(hc), true);
        self.set(&format!("sc:{f}"), v.int(sc), true);
        Ok(f)
    }

    /// `\patterns` (§960–§965): the words as §962 reads them (letters
    /// through `\lccode`, values as digits), entered when a paragraph
    /// needs them.
    fn new_patterns(&mut self) -> R<()> {
        self.scan_left_brace()?;
        let lang = phitex_typeset::hyph::cur_lang(self.ipar(Par::language)?);
        let mut words: Vec<ValueId> = Vec::new();
        let mut word = String::new();
        let mut digit_sensed = false;
        let v = self.v;
        loop {
            self.get_x_token()?;
            match self.cur.cmd {
                cmd::LETTER | cmd::OTHER_CHAR => {
                    let c = u32::try_from(self.cur.chr).unwrap_or(0);
                    if digit_sensed || !(48..=57).contains(&c) {
                        let code = if c == u32::from(b'.') {
                            c
                        } else {
                            u32::try_from(self.int(&format!("lc:{c}"))?).unwrap_or(0)
                        };
                        word.push(char::from_u32(code).unwrap_or('\0'));
                        digit_sensed = false;
                    } else {
                        word.push(char::from_u32(c).unwrap_or('0'));
                        digit_sensed = true;
                    }
                }
                cmd::SPACER | cmd::RIGHT_BRACE => {
                    if !word.is_empty() {
                        words.push(v.str(&std::mem::take(&mut word)));
                    }
                    digit_sensed = false;
                    if self.cur.cmd == cmd::RIGHT_BRACE {
                        break;
                    }
                }
                _ => {
                    // "Bad \patterns"
                }
            }
        }
        let old = self.get_loc(crate::fields::cell::PATTERNS)?;
        let mut all = old.map(|x| list_of(v, x)).unwrap_or_default();
        all.push(v.list(vec![v.int(i64::from(lang)), v.str("p"), v.list(words)]));
        self.set_cell(crate::fields::cell::PATTERNS, v.list(all), true);
        Ok(())
    }

    /// `\hyphenation` (§934–§939).
    fn new_hyph_exceptions(&mut self) -> R<()> {
        self.scan_left_brace()?;
        let lang = phitex_typeset::hyph::cur_lang(self.ipar(Par::language)?);
        let v = self.v;
        let mut words: Vec<ValueId> = Vec::new();
        let mut word = String::new();
        loop {
            self.get_x_token()?;
            match self.cur.cmd {
                cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN => {
                    if self.cur.chr == i32::from(b'-') {
                        word.push('-');
                    } else {
                        let c = u32::try_from(self.cur.chr).unwrap_or(0);
                        let lc = self.int(&format!("lc:{c}"))?;
                        if lc != 0 {
                            word.push(
                                char::from_u32(u32::try_from(lc).unwrap_or(0)).unwrap_or('?'),
                            );
                        }
                    }
                }
                cmd::CHAR_NUM => {
                    let c = self.scan_char_num()?;
                    let lc = self.int(&format!("lc:{c}"))?;
                    if lc != 0 {
                        word.push(char::from_u32(u32::try_from(lc).unwrap_or(0)).unwrap_or('?'));
                    }
                }
                cmd::SPACER | cmd::RIGHT_BRACE => {
                    if !word.is_empty() {
                        words.push(v.str(&std::mem::take(&mut word)));
                    }
                    if self.cur.cmd == cmd::RIGHT_BRACE {
                        break;
                    }
                }
                _ => {}
            }
        }
        let old = self.get_loc(crate::fields::cell::PATTERNS)?;
        let mut all = old.map(|x| list_of(v, x)).unwrap_or_default();
        all.push(v.list(vec![v.int(i64::from(lang)), v.str("h"), v.list(words)]));
        self.set_cell(crate::fields::cell::PATTERNS, v.list(all), true);
        Ok(())
    }

    /// `open_or_close_in` (§1275).
    fn open_or_close_in(&mut self) -> R<()> {
        let c = self.cur.chr;
        let n = self.scan_four_bit_int()?;
        let v = self.v;
        let field = format!("read:{n}");
        if self.get(&field)?.is_some_and(|x| x != ValueId::UNIT) {
            self.set(&field, ValueId::UNIT, true);
        }
        if c != 0 {
            self.scan_optional_equals()?;
            let name = self.scan_file_name()?;
            let key = if name.contains('.') {
                name.clone()
            } else {
                format!("{name}.tex")
            };
            let Some(x) = self.read_file(&key)? else {
                return Ok(());
            };
            let xs = list_of(v, x);
            if xs.len() >= 2 {
                let text = str_of(v, xs[1]);
                let lines: Vec<ValueId> = crate::expand::split_lines(&text)
                    .iter()
                    .map(|l| v.str(l))
                    .collect();
                let st = v.list(vec![v.int(1), v.list(lines)]);
                self.set(&field, st, true);
            }
        }
        Ok(())
    }

    /// The next line of the open stream whose state `st` field `field`
    /// holds (`[state, lines, position]`): the position moves on, or at
    /// the end (`None`) the stream closes (§486). The lines are kept as
    /// they were read in; only the position changes per line.
    pub(crate) fn next_read_line(&mut self, field: &str, st: ValueId) -> Option<String> {
        let v = self.v;
        let (lines, pos) = match &*v.get(st) {
            Value::List(xs) => (
                xs.get(1).copied(),
                xs.get(2).map_or(0, |&p| crate::val::int_of(v, p)),
            ),
            _ => (None, 0),
        };
        let at = usize::try_from(pos).unwrap_or(0);
        let line = lines.and_then(|l| match &*v.get(l) {
            Value::List(xs) => xs.get(at).copied(),
            _ => None,
        });
        if let (Some(l), Some(lines)) = (line, lines) {
            let next = v.list(vec![v.int(2), lines, v.int(pos + 1)]);
            self.set(field, next, true);
            Some(str_of(v, l))
        } else {
            self.set(field, ValueId::UNIT, true);
            None
        }
    }

    /// `read_toks` (§482–§486): the lines of stream `n` up to balanced
    /// braces.
    fn read_toks(&mut self, n: i32) -> R<Vec<Tok>> {
        let m = if (0..=15).contains(&n) { n } else { 16 };
        let field = format!("read:{m}");
        let mut out = Vec::new();
        let mut align: i64 = 0;
        loop {
            let st = self.get(&field)?;
            let open = st.filter(|&x| x != ValueId::UNIT);
            let Some(st) = open else {
                // §484: from the terminal: a fatal error in batch mode;
                // a build has no terminal, so in every mode (TeX at the
                // end of its terminal input stops the job too, §360)
                self.log(
                    "error",
                    "*** (cannot \\read from terminal in nonstop modes)",
                )?;
                return Err(crate::run::Halt::Fatal);
            };
            let line = self.next_read_line(&field, st);
            let text = line.unwrap_or_default();
            let e = i64::from(self.ipar(Par::endlinechar)?);
            let frames = vec![crate::run::src_frame(&text, e)];
            let saved = std::mem::take(&mut self.frames);
            let saved_eof = self.eof;
            self.eof = true;
            self.frames.extend(frames);
            let mut res = Ok(());
            loop {
                match self.next_tok() {
                    Ok(Some(t)) => {
                        if !t.is_cs_or_active() {
                            match t.cat() {
                                Some(1) => align += 1,
                                Some(2) => align -= 1,
                                _ => {}
                            }
                        }
                        if align < 0 {
                            // §485: an unmatched } aborts the line
                            align = 0;
                            while let Ok(Some(_)) = self.next_tok() {}
                            break;
                        }
                        out.push(t);
                    }
                    Ok(None) => break,
                    Err(e) => {
                        res = Err(e);
                        break;
                    }
                }
            }
            self.frames = saved;
            self.eof = saved_eof;
            res?;
            if align == 0 {
                return Ok(out);
            }
            if self.get(&field)?.is_none_or(|x| x == ValueId::UNIT) {
                return Ok(out);
            }
        }
    }

    /// `show_whatever` (§1293): shown in the log.
    fn show_whatever(&mut self) -> R<()> {
        if self.cur.chr >= 4 {
            return crate::etex::show(self);
        }
        let mut s = String::from("> ");
        match self.cur.chr {
            0 => {
                self.get_token()?;
                let esc = i64::from(self.ipar(Par::escapechar)?);
                if self.cur.is_cs() {
                    crate::print::sprint_cs(&mut s, self.cur.tok, esc);
                    s.push('=');
                }
                let (m, c, h) = (self.cur.mean.clone(), self.cur.cmd, self.cur.chr);
                self.print_meaning(&mut s, &m, c, h)?;
            }
            2 => {
                let t = self.the_toks()?;
                self.show_toks_into(&mut s, &t)?;
            }
            1 => {
                let _ = self.scan_eight_bit_int()?;
            }
            _ => {}
        }
        s.push('.');
        self.log("show", &s)
    }

    /// `do_extension` (§1348).
    fn do_extension(&mut self, md: &crate::build::Mode) -> R<()> {
        let v = self.v;
        match self.cur.chr {
            0 | 2 => {
                // \openout, \closeout (§1351, §1353)
                let c = self.cur.chr;
                let n = self.scan_four_bit_int()?;
                let name = if c == 0 {
                    self.scan_optional_equals()?;
                    self.scan_file_name()?
                } else {
                    String::new()
                };
                let w = v.list(vec![
                    v.str(if c == 0 { "open" } else { "close" }),
                    v.int(i64::from(n)),
                    v.str(&name),
                ]);
                let node = v.list(vec![v.int(crate::nodes::T_WHATSIT), w]);
                self.append(md.lid, node);
            }
            1 => {
                // \write (§1352)
                let k = self.scan_int()?;
                let (_, body) = self.scan_toks(false, false)?;
                let w = v.list(vec![
                    v.str("write"),
                    v.int(i64::from(k)),
                    toks_value(v, &body),
                ]);
                let node = v.list(vec![v.int(crate::nodes::T_WHATSIT), w]);
                self.append(md.lid, node);
            }
            3 => {
                // \special (§1354)
                let (_, body) = self.scan_toks(false, true)?;
                let mut s = String::new();
                self.show_toks_into(&mut s, &body)?;
                let w = v.list(vec![v.str("special"), v.str(&s)]);
                let node = v.list(vec![v.int(crate::nodes::T_WHATSIT), w]);
                self.append(md.lid, node);
            }
            4 => {
                // \immediate (§1375)
                self.get_x_token()?;
                if self.cur.cmd == cmd::EXTENSION && self.cur.chr <= 2 {
                    match self.cur.chr {
                        1 => {
                            let k = self.scan_int()?;
                            let (_, body) = self.scan_toks(false, false)?;
                            self.write_out(k, &body)?;
                        }
                        0 => {
                            let n = self.scan_four_bit_int()?;
                            self.scan_optional_equals()?;
                            let name = self.scan_file_name()?;
                            self.open_out(i64::from(n), &name)?;
                        }
                        _ => {
                            let n = self.scan_four_bit_int()?;
                            self.set(&format!("write:{n}"), ValueId::UNIT, true);
                        }
                    }
                } else {
                    // pdfTeX §1375: `\immediate\pdfobj`, `\pdfxform`, `\pdfximage`
                    if self.cur.cmd == cmd::EXTENSION {
                        return crate::pdfprim::immediate(self, md);
                    }
                    self.back_cur();
                }
            }
            _ => {
                // \setlanguage (§1377)
                let _ = self.scan_int()?;
            }
        }
        Ok(())
    }

    /// `write_out` (§1370–§1371): the tokens expanded, to the log.
    pub(crate) fn write_out(&mut self, k: i32, body: &[Tok]) -> R<()> {
        let s = self.expand_late(body)?;
        let open = (0..16).contains(&k)
            && self
                .get(&format!("write:{k}"))?
                .is_some_and(|x| x != ValueId::UNIT);
        if open {
            let v = self.v;
            let e = v.list(vec![v.int(i64::from(k)), v.str(&s)]);
            let su = v.seq_unit(e);
            self.contribute("writes", su);
            // the file's text (§1370: `print` with `\newlinechar`, then
            // `print_ln`)
            let name = self
                .get(&format!("write:{k}"))?
                .and_then(|x| list_of(v, x).first().map(|&n| str_of(v, n)))
                .unwrap_or_default();
            let nl = i64::from(self.ipar(Par::newlinechar)?);
            let mut line: String = s
                .chars()
                .map(|c| {
                    if i64::from(u32::from(c)) == nl {
                        '\n'
                    } else {
                        c
                    }
                })
                .collect();
            line.push('\n');
            // a file whose read-back only defines: the definitions this
            // line's read-back makes go with it (`wdef:<field>`)
            let e = if self
                .cx
                .is_loopback(self.keyed(crate::fields::Fam::Wkeys, &name))
            {
                let made = self.interpret_line(&line)?;
                v.list(vec![v.str(&name), v.str(&line), made])
            } else {
                v.list(vec![v.str(&name), v.str(&line)])
            };
            self.contribute_stream(e);
        } else {
            self.log("wlog", &s)?;
        }
        Ok(())
    }

    /// `\openout` done (§1374): the stream names the file (`.tex` if it
    /// has no extension), which starts empty: its text accumulates in
    /// the `streams` sequence, and `wopen:<name>` tells later readers
    /// of the file that this run wrote it.
    pub(crate) fn open_out(&mut self, n: i64, name: &str) -> R<()> {
        let v = self.v;
        let base = name.rsplit('/').next().unwrap_or(name);
        let name = if base.contains('.') {
            name.to_owned()
        } else {
            format!("{name}.tex")
        };
        self.written_decl(&name)?;
        self.set(&format!("write:{n}"), v.list(vec![v.str(&name)]), true);
        self.set(&format!("wopen:{name}"), v.int(1), true);
        self.contribute_stream(v.list(vec![v.str(&name)]));
        Ok(())
    }

    /// The file `key` as TeX reads it here: if this run opened it for
    /// writing before this point, the text written to it so far (TeX
    /// reads what it wrote, as LaTeX's `\enddocument` reads the `.aux`
    /// file); else the client's `file:` field (a loop-back field for a
    /// file the job writes: the previous round's text).
    pub(crate) fn read_file(&mut self, key: &str) -> R<Option<ValueId>> {
        let v = self.v;
        if self.get(&format!("wopen:{key}"))?.is_none() {
            return self.file(key);
        }
        let f = v.field("streams");
        let before = self.cx.acc_range(f, None)?;
        let mut all = v.seq_to_vec(before);
        all.extend(
            self.contribs
                .iter()
                .filter(|(g, _)| *g == f)
                .flat_map(|(_, x)| v.seq_to_vec(*x)),
        );
        let mut text = String::new();
        for e in all {
            let xs = list_of(v, crate::api::stream_entry(v, e));
            if xs.first().map(|&n| str_of(v, n)).as_deref() != Some(key) {
                continue;
            }
            match xs.get(1) {
                Some(&t) => text.push_str(&str_of(v, t)),
                None => text.clear(),
            }
        }
        Ok(Some(v.list(vec![v.str(&format!("./{key}")), v.str(&text)])))
    }

    /// §1371: a token list expanded as `\write` expands it at
    /// `\shipout` (also `\pdfliteral shipout`), shown as a string.
    pub(crate) fn expand_late(&mut self, body: &[Tok]) -> R<String> {
        let mut toks = vec![Tok::ch(1, u32::from(b'{'))];
        toks.extend(body.iter().copied());
        toks.push(Tok::ch(2, u32::from(b'}')));
        toks.push(Tok::cs("\u{10FFFE}endwrite"));
        self.push_toks(toks);
        let (_, expanded) = self.scan_toks(false, true)?;
        self.get_token()?;
        let mut s = String::new();
        self.show_toks_into(&mut s, &expanded)?;
        Ok(s)
    }

    /// The input left as one source line, for `\inputlineno`.
    pub(crate) fn input_line_no(&mut self) -> R<i32> {
        let lines = self.v.field("lines");
        let x = self.cx.acc_range(lines, None)?;
        Ok(i32::try_from(int_of(self.v, x) + self.lines_entered).unwrap_or(0))
    }
}

/// §1239: the sum of two glue specifications.
pub(crate) fn add_glue(q: &Glue, r: &Glue) -> Glue {
    let mut g = *q;
    g.width += r.width;
    if g.stretch == 0 {
        g.stretch_order = phitex_typeset::glue::Order::Normal;
    }
    if g.stretch_order == r.stretch_order {
        g.stretch += r.stretch;
    } else if g.stretch_order < r.stretch_order && r.stretch != 0 {
        g.stretch = r.stretch;
        g.stretch_order = r.stretch_order;
    }
    if g.shrink == 0 {
        g.shrink_order = phitex_typeset::glue::Order::Normal;
    }
    if g.shrink_order == r.shrink_order {
        g.shrink += r.shrink;
    } else if g.shrink_order < r.shrink_order && r.shrink != 0 {
        g.shrink = r.shrink;
        g.shrink_order = r.shrink_order;
    }
    g
}
