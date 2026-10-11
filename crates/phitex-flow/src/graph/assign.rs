//! Assignments (tex.web 1208-1280): each is one node, whose results are
//! its definitions (`Exec::define`). Prefixes are nodes that hand their
//! flags to the next node through the reader state (`RState::global`,
//! `long`, `outer`); `\afterassignment`'s token is handed the same way.
//! Also `\message`, whose result is a string for the log (the output
//! streams are in `io.rs`), `\read` (a definition, also in `io.rs`), and
//! `\uppercase`/`\lowercase`, which rewrite the input.

use std::sync::Arc;

use super::Exec;
use super::scan::{mult_and_add, nx_plus_y, x_over_n};
use crate::val::{Code, Glue, IntPar, Key, Macro, Mat, Meaning, Prim, Reg, Shorthand, Tok, Val, Xe};

/// Whether a command takes prefixes: an assignment (tex.web 1210,
/// `max_non_prefixed_command`).
pub(super) fn prefixable(m: &Meaning) -> bool {
    matches!(
        m,
        Meaning::Prim(
            Prim::Def
            | Prim::Gdef
            | Prim::Edef
            | Prim::Xdef
            | Prim::Let
            | Prim::Futurelet
            | Prim::Shorthand(_)
            | Prim::Code(_)
            | Prim::IntPar(_)
            | Prim::DimenPar(_)
            | Prim::GluePar(_)
            | Prim::TokPar(_)
            | Prim::Register(_)
            | Prim::Advance
            | Prim::Multiply
            | Prim::Divide
            | Prim::Global
            | Prim::Long
            | Prim::Outer
            | Prim::Parshape
            | Prim::FontDimen
            | Prim::Font
            | Prim::SpaceFactor
            | Prim::PrevDepth
            | Prim::PrevGraf
            | Prim::PageInt(_)
            | Prim::PageDimen(_)
            | Prim::BoxDimen(_)
            | Prim::Interaction(_)
            | Prim::DefFamily(_)
            | Prim::Read
            | Prim::Protected
            | Prim::ReadLine
            | Prim::PenShape(_)
            | Prim::Mat(Mat::SetBox | Mat::FontInt(_))
            | Prim::HyphData(_)
            | Prim::PdfFont(2 | 3)
            | Prim::Xe(Xe::DefCode(_) | Xe::MathCharDef(_) | Xe::InterCharToks)
        ) | Meaning::CountRef(_)
            | Meaning::DimenRef(_)
            | Meaning::SkipRef(_)
            | Meaning::MuskipRef(_)
            | Meaning::ToksRef(_)
            | Meaning::Font(_)
    )
}

/// The register an assignment's target names: its key and level.
enum Target {
    Int(Key),
    Dimen(Key),
    Glue(Key),
    Mu(Key),
}

impl Exec<'_> {
    /// `\global`, `\long`, `\outer`, e-TeX's `\protected`: the flags go to
    /// the next node, which must be an assignment (tex.web 1211).
    #[allow(clippy::fn_params_excessive_bools, reason = "TeX's prefixes")]
    pub(super) fn prefix(&mut self, p: Prim, global: bool, long: bool, outer: bool, protected: bool) {
        let (mut g, mut l, mut o, mut pr) = (global, long, outer, protected);
        match p {
            Prim::Global => g = true,
            Prim::Long => l = true,
            Prim::Protected => pr = true,
            _ => o = true,
        }
        let Some((t, m)) = self.nonblank_nonrelax() else {
            return;
        };
        if !prefixable(&m) {
            self.err(format!("You can't use a prefix with `{}'", self.shown(&t)));
            self.back(t);
            return;
        }
        self.st.global = g;
        self.st.long = l;
        self.st.outer = o;
        self.st.protected = pr;
        self.back(t);
    }

    /// Defines `key`, globally if asked (tex.web 1214).
    fn assign(&mut self, key: Key, val: Val, global: bool) {
        self.define(key, val, global);
    }

    /// An assignment, with its prefixes (tex.web 1211-1214).
    #[allow(clippy::fn_params_excessive_bools, reason = "TeX's prefixes")]
    pub(super) fn prefixed(&mut self, t: &Tok, m: &Meaning, global: bool, long: bool, outer: bool, protected: bool) {
        let is_def = matches!(m, Meaning::Prim(Prim::Def | Prim::Gdef | Prim::Edef | Prim::Xdef));
        if (long || outer || protected) && !is_def {
            self.err(format!("You can't use `\\long' or `\\outer' with `{}'", self.shown(t)));
        }
        let gd = self.int_par(IntPar::Globaldefs);
        let mut global = global;
        if gd > 0 {
            global = true;
        } else if gd < 0 {
            global = false;
        }
        match m {
            Meaning::Prim(p) => match *p {
                Prim::Def | Prim::Gdef | Prim::Edef | Prim::Xdef => {
                    let g = global || (matches!(p, Prim::Gdef | Prim::Xdef) && gd >= 0);
                    self.def(g, matches!(p, Prim::Edef | Prim::Xdef), long, outer, protected);
                }
                Prim::Let | Prim::Futurelet => self.let_(*p == Prim::Futurelet, global),
                Prim::Shorthand(s) => self.shorthand(s, global),
                Prim::Code(c) => self.code_assign(c, global),
                Prim::Xe(Xe::DefCode(k)) => self.xe_def_code(k, global),
                Prim::Xe(Xe::MathCharDef(full)) => self.xe_math_char_def(full, global),
                Prim::Xe(Xe::InterCharToks) => self.xe_interchar_assign(global),
                Prim::IntPar(q) => {
                    self.optional_equals();
                    let v = self.scan_int();
                    self.assign(Key::IntPar(q), Val::Int(v), global);
                }
                Prim::DimenPar(q) => {
                    self.optional_equals();
                    let v = self.scan_dimen();
                    self.assign(Key::DimenPar(q), Val::Int(v), global);
                }
                Prim::GluePar(q) => {
                    self.optional_equals();
                    let g = self.scan_glue(q.is_mu());
                    self.assign(Key::GluePar(q), Val::Glue(Arc::new(g)), global);
                }
                Prim::TokPar(q) => self.toks_assign(Key::TokPar(q), q == crate::val::TokPar::Output, global),
                Prim::Register(Reg::Toks) => {
                    let n = self.scan_eight_bit_int();
                    self.toks_assign(Key::Toks(n), false, global);
                }
                Prim::Register(_) | Prim::Advance | Prim::Multiply | Prim::Divide => {
                    self.register_command(t, m, global);
                }
                Prim::Parshape => {
                    self.optional_equals();
                    let n = self.scan_int();
                    let mut v = Vec::new();
                    for _ in 0..n.max(0) {
                        let i = self.scan_dimen();
                        let l = self.scan_dimen();
                        v.push((i, l));
                    }
                    self.assign(Key::Parshape, Val::Parshape(Arc::new(v)), global);
                }
                Prim::FontDimen => {
                    let k = self.find_font_dimen();
                    self.optional_equals();
                    let v = self.scan_dimen();
                    if let Some(k) = k {
                        // (font dimensions are always global, tex.web 1253)
                        self.assign(k, Val::Int(v), true);
                    }
                }
                Prim::SpaceFactor | Prim::PrevDepth => {
                    // (the list being built's: a field of the reader state,
                    // tex.web 1243)
                    let want = if *p == Prim::SpaceFactor { super::Mode::H } else { super::Mode::V };
                    if self.st.mode != want {
                        self.err(format!("You can't use `\\{}' in this mode", p.name()));
                        return;
                    }
                    self.optional_equals();
                    if want == super::Mode::V {
                        // (and the list's gatherer sets it where it is in the
                        // list: the depth before is material, CORE.md)
                        self.st.prev_depth = self.scan_dimen();
                        self.vemit.push(crate::item::Item::PrevDepth(self.st.prev_depth));
                    } else {
                        let v = self.scan_int();
                        if v <= 0 || v > 32767 {
                            self.err("Bad space factor".into());
                        } else {
                            self.st.space_factor = v;
                        }
                    }
                }
                Prim::PrevGraf => {
                    self.optional_equals();
                    let v = self.scan_int();
                    if v < 0 {
                        self.err("Bad \\prevgraf".into());
                    } else {
                        self.st.prev_graf = v;
                    }
                }
                Prim::BoxDimen(c) => self.alter_box_dimen(c),
                Prim::Font => self.new_font(global),
                Prim::DefFamily(size) => {
                    // (tex.web 1234)
                    let fam = self.scan_fam_num();
                    self.optional_equals();
                    let f = self.scan_font_ident();
                    self.assign(Key::FamFont(size, fam), Val::Font(f), global);
                }
                Prim::Mat(Mat::SetBox) => self.setbox(global),
                Prim::Mat(Mat::FontInt(k)) => self.assign_font_int(k),
                Prim::Read => self.read_to_cs(global),
                // (the mode is a key; `new_interaction` begins a line)
                Prim::Interaction(k) => self.new_interaction(i32::from(k)),
                Prim::PageInt(2) => self.interaction_mode_assign(),
                Prim::PageInt(_) | Prim::PageDimen(_) => self.alter_page(*p),
                Prim::ReadLine => self.read_line(global),
                Prim::PenShape(k) => self.penalties_assign(k, global),
                Prim::HyphData(pat) => self.hyph_data(pat),
                Prim::PdfFont(2) => self.pdf_copy_font(global),
                Prim::PdfFont(3) => self.letterspace_font(global),
                _ => self.err(format!("\\{} is not supported yet", p.name())),
            },
            Meaning::CountRef(_) | Meaning::DimenRef(_) | Meaning::SkipRef(_) | Meaning::MuskipRef(_) => {
                self.register_command(t, m, global);
            }
            Meaning::ToksRef(n) => self.toks_assign(Key::Toks(*n), false, global),
            Meaning::Font(f) => self.select_font(f.clone(), global),
            _ => {}
        }
        // (tex.web 1269)
        if let Some(a) = self.st.after_assignment.take() {
            self.back(*a);
        }
    }

    /// The control sequence an assignment defines (tex.web 1215).
    pub(super) fn r_token(&mut self) -> Option<Key> {
        loop {
            let t = self.next_raw()?;
            if t.is_char(10, ' ') {
                continue;
            }
            if let Some(k) = t.key() {
                return Some(k);
            }
            self.err("Missing control sequence inserted".into());
            self.back(t);
            return None;
        }
    }

    /// `\def`, `\gdef`, `\edef`, `\xdef` (tex.web 1218, 473-479).
    #[allow(clippy::fn_params_excessive_bools, reason = "TeX's prefixes")]
    fn def(&mut self, global: bool, expand: bool, long: bool, outer: bool, protected: bool) {
        let Some(key) = self.r_token() else { return };
        let w = Some(match &key {
            Key::Active(c) => Tok::Active(*c),
            Key::Cs(c) => Tok::Cs(*c),
            _ => self.cmd.clone(),
        });
        self.scanning(super::Scanning::Defining, w, |x| x.def_in(key, global, expand, long, outer, protected));
    }

    #[allow(clippy::fn_params_excessive_bools, reason = "TeX's prefixes")]
    fn def_in(&mut self, key: Key, global: bool, expand: bool, long: bool, outer: bool, protected: bool) {
        let mut params = Vec::new();
        let mut n = 0u8;
        let mut hash_brace = false;
        loop {
            let Some(t) = self.next_raw() else { return };
            match t.cat() {
                Some(1) => break,
                Some(2) => {
                    self.err("Missing { inserted".into());
                    self.define(
                        key,
                        Val::Meaning(Meaning::Macro(Arc::new(Macro { params, body: Arc::from([]), long, outer, protected }))),
                        global,
                    );
                    return;
                }
                Some(6) => match self.next_raw() {
                    Some(b) if b.cat() == Some(1) => {
                        hash_brace = true;
                        params.push(b);
                        break;
                    }
                    Some(x) => {
                        if n == 9 {
                            self.err("You already have nine parameters".into());
                            continue;
                        }
                        n += 1;
                        if !x.is_char(12, char::from(b'0' + n)) {
                            self.err("Parameters must be numbered consecutively".into());
                            self.back(x);
                        }
                        params.push(Tok::Param(n, if let Tok::Char(_, c) = t { c } else { '#' }));
                    }
                    None => return,
                },
                _ => params.push(t),
            }
        }
        let mut body = if expand { self.expand_text(Some(n)) } else { self.def_body(n) };
        if hash_brace {
            body.push(Tok::Char(1, '{'));
        }
        self.define(
            key,
            Val::Meaning(Meaning::Macro(Arc::new(Macro { params, body: body.into(), long, outer, protected }))),
            global,
        );
    }

    /// A definition's body, unexpanded (tex.web 477, 479).
    fn def_body(&mut self, n: u8) -> Vec<Tok> {
        let mut body = Vec::new();
        let mut depth = 1;
        loop {
            if self.holding(body.len()) {
                return body;
            }
            let Some(t) = self.next_raw() else {
                self.err("File ended while scanning a definition".into());
                return body;
            };
            match t.cat() {
                Some(1) => depth += 1,
                Some(2) => {
                    depth -= 1;
                    if depth == 0 {
                        return body;
                    }
                }
                Some(6) => {
                    match self.next_raw() {
                        Some(x) if x.cat() == Some(6) => body.push(x),
                        Some(Tok::Char(12, d)) if d.is_ascii_digit() && d != '0' && (d as u8 - b'0') <= n => {
                            // (an `out_param` token: no character of its own, tex.web 476)
                            body.push(Tok::Param(d as u8 - b'0', '#'));
                        }
                        other => {
                            self.err("Illegal parameter number in definition".into());
                            if let Some(x) = other {
                                self.back(x);
                            }
                            body.push(t);
                        }
                    }
                    continue;
                }
                _ => {}
            }
            body.push(t);
        }
    }

    /// `\let`, `\futurelet` (tex.web 1221).
    fn let_(&mut self, future: bool, global: bool) {
        let Some(key) = self.r_token() else { return };
        let m = if future {
            let Some(t1) = self.next_raw() else { return };
            let Some(t2) = self.next_raw() else {
                self.back(t1);
                return;
            };
            let m = self.raw_meaning(&t2);
            self.back(t2);
            self.back(t1);
            m
        } else {
            let mut t = self.next_raw();
            while let Some(x) = &t {
                if matches!(self.raw_meaning(x), Meaning::Char(10, _)) {
                    t = self.next_raw();
                } else {
                    break;
                }
            }
            if t.as_ref().is_some_and(|x| x.is_char(12, '=')) {
                t = self.next_raw();
                if t.as_ref().is_some_and(|x| matches!(self.raw_meaning(x), Meaning::Char(10, _))) {
                    t = self.next_raw();
                }
            }
            let Some(t) = t else { return };
            self.raw_meaning(&t)
        };
        self.define(key, Val::Meaning(m), global);
    }

    /// The meaning of a token as `get_token` sees it: a `\noexpand`ed name
    /// that would expand means `\relax`.
    fn raw_meaning(&mut self, t: &Tok) -> Meaning {
        let m = self.tok_meaning(t);
        if self.noexp && super::expand::expands(&m) { Meaning::Prim(Prim::Relax) } else { m }
    }

    /// `\chardef` and friends (tex.web 1224).
    fn shorthand(&mut self, s: Shorthand, global: bool) {
        let Some(key) = self.r_token() else { return };
        self.define(key.clone(), Val::Meaning(Meaning::Prim(Prim::Relax)), global);
        self.optional_equals();
        let m = match s {
            Shorthand::Char => {
                let c = if self.xetex { self.scan_usv_num() } else { self.scan_char_num() };
                Meaning::CharGiven(c as u32)
            }
            Shorthand::MathChar => Meaning::MathCharGiven(self.scan_fifteen_bit_int() as u32),
            _ => {
                let n = self.scan_eight_bit_int();
                match s {
                    Shorthand::Count => Meaning::CountRef(n),
                    Shorthand::Dimen => Meaning::DimenRef(n),
                    Shorthand::Skip => Meaning::SkipRef(n),
                    Shorthand::Muskip => Meaning::MuskipRef(n),
                    _ => Meaning::ToksRef(n),
                }
            }
        };
        self.define(key, Val::Meaning(m), global);
    }

    /// `\catcode` and the other code tables (tex.web 1232).
    fn code_assign(&mut self, c: Code, global: bool) {
        // (XeTeX: any Unicode scalar value, xetex.web's `def_code`)
        let n = self.scan_code_char();
        let ch = super::xetex::usv(n);
        self.optional_equals();
        let mut v = self.scan_int();
        let max = match c {
            Code::Cat => 15,
            Code::Math => 0o100_000,
            Code::Lc | Code::Uc if self.xetex => 0x10_FFFF,
            Code::Lc | Code::Uc => 255,
            Code::Sf => 0o77777,
            Code::Del => 0o77_777_777,
        };
        if (v < 0 && c != Code::Del) || v > max {
            self.err(format!(
                "Invalid code ({v}), should be {}{max}",
                if c == Code::Del { "at most " } else { "in the range 0.." }
            ));
            v = 0;
        }
        if self.xetex {
            v = self.xe_code_in(c, ch, v);
        }
        let val = if c == Code::Cat { Val::Cat(v as u8) } else { Val::Int(v) };
        self.assign(Self::code_key(c, ch), val, global);
    }

    /// A token list assignment (tex.web 1226-1227).
    pub(super) fn toks_assign(&mut self, key: Key, output: bool, global: bool) {
        self.optional_equals();
        let Some((t, m)) = self.nonblank_nonrelax() else {
            return;
        };
        // (another list: copied)
        let src = match m {
            Meaning::Prim(Prim::Register(Reg::Toks)) => Some(Key::Toks(self.scan_eight_bit_int())),
            Meaning::Prim(Prim::TokPar(q)) => Some(Key::TokPar(q)),
            Meaning::ToksRef(n) => Some(Key::Toks(n)),
            _ => None,
        };
        if let Some(src) = src {
            let v = self.get(&src);
            self.assign(key, v, global);
            return;
        }
        self.back(t);
        let mut l = self.scan_toks_raw();
        if output && !l.is_empty() {
            l.insert(0, Tok::Char(1, '{'));
            l.push(Tok::Char(2, '}'));
        }
        self.assign(key, Val::Toks(Arc::new(l)), global);
    }

    /// The register `\advance` and friends name (tex.web 1237).
    fn target(&mut self, m: &Meaning) -> Option<Target> {
        Some(match m {
            Meaning::Prim(Prim::IntPar(q)) => Target::Int(Key::IntPar(*q)),
            Meaning::Prim(Prim::DimenPar(q)) => Target::Dimen(Key::DimenPar(*q)),
            Meaning::Prim(Prim::GluePar(q)) => {
                if q.is_mu() {
                    Target::Mu(Key::GluePar(*q))
                } else {
                    Target::Glue(Key::GluePar(*q))
                }
            }
            Meaning::CountRef(n) => Target::Int(Key::Count(*n)),
            Meaning::DimenRef(n) => Target::Dimen(Key::Dimen(*n)),
            Meaning::SkipRef(n) => Target::Glue(Key::Skip(*n)),
            Meaning::MuskipRef(n) => Target::Mu(Key::Muskip(*n)),
            Meaning::Prim(Prim::Register(r)) if *r != Reg::Toks => {
                let n = self.scan_eight_bit_int();
                match r {
                    Reg::Count => Target::Int(Key::Count(n)),
                    Reg::Dimen => Target::Dimen(Key::Dimen(n)),
                    Reg::Skip => Target::Glue(Key::Skip(n)),
                    _ => Target::Mu(Key::Muskip(n)),
                }
            }
            _ => return None,
        })
    }

    /// A register assignment, `\advance`, `\multiply`, `\divide`
    /// (tex.web 1236-1240).
    fn register_command(&mut self, t: &Tok, m: &Meaning, global: bool) {
        let op = match m {
            Meaning::Prim(p @ (Prim::Advance | Prim::Multiply | Prim::Divide)) => Some(*p),
            _ => None,
        };
        let target = match op {
            None => self.target(m),
            Some(p) => {
                let Some((x, xm)) = self.next_x_m(false) else {
                    return;
                };
                let tg = self.target(&xm);
                if tg.is_none() {
                    self.err(format!("You can't use `{}' after \\{}", self.shown(&x), p.name()));
                    return;
                }
                tg
            }
        };
        let Some(target) = target else {
            self.err(format!("You can't use `{}' here", self.shown(t)));
            return;
        };
        if op.is_none() {
            self.optional_equals();
        } else {
            let _ = self.keyword("by");
        }
        let (key, val) = match op {
            None | Some(Prim::Advance) => match target {
                Target::Int(k) => {
                    let mut v = self.scan_int();
                    if op.is_some() {
                        v = v.wrapping_add(self.int(&k));
                    }
                    (k, Val::Int(v))
                }
                Target::Dimen(k) => {
                    let mut v = self.scan_dimen();
                    if op.is_some() {
                        v = v.wrapping_add(self.int(&k));
                    }
                    (k, Val::Int(v))
                }
                tg @ (Target::Glue(_) | Target::Mu(_)) => {
                    let (k, mu) = match tg {
                        Target::Glue(k) => (k, false),
                        Target::Mu(k) => (k, true),
                        _ => unreachable!(),
                    };
                    let mut g = self.scan_glue(mu);
                    if op.is_some() {
                        let r = match self.get(&k) {
                            Val::Glue(r) => *r,
                            _ => Glue::default(),
                        };
                        g = add_glue(g, r);
                    }
                    (k, Val::Glue(Arc::new(g)))
                }
            },
            Some(p) => {
                let n = self.scan_int();
                let mul = p == Prim::Multiply;
                let res = match target {
                    Target::Int(k) | Target::Dimen(k) => {
                        let is_int = matches!(k, Key::Count(_) | Key::IntPar(_));
                        let x = self.int(&k);
                        let r = if !mul {
                            x_over_n(x, n)
                        } else if is_int {
                            mult_and_add(x, n, 0, 0o17_777_777_777)
                        } else {
                            nx_plus_y(x, n, 0)
                        };
                        r.map(|v| (k, Val::Int(v)))
                    }
                    Target::Glue(k) | Target::Mu(k) => {
                        let s = match self.get(&k) {
                            Val::Glue(s) => *s,
                            _ => Glue::default(),
                        };
                        let f = |v: i32| {
                            if mul { nx_plus_y(v, n, 0) } else { x_over_n(v, n) }
                        };
                        match (f(s.width), f(s.stretch), f(s.shrink)) {
                            (Some(w), Some(st), Some(sh)) => {
                                Some((k, Val::Glue(Arc::new(Glue { width: w, stretch: st, shrink: sh, ..s }))))
                            }
                            _ => None,
                        }
                    }
                };
                let Some(r) = res else {
                    self.err("Arithmetic overflow".into());
                    return;
                };
                r
            }
        };
        self.assign(key, val, global);
    }

    // ------------------------------------------------------------ the log

    /// `\message`, `\errmessage` (tex.web 1279): the expanded text, shown.
    pub(super) fn message(&mut self, error: bool) {
        let toks = self.scan_toks_x();
        let mut s = String::new();
        self.show_toks(&mut s, &toks);
        if error {
            // (tex.web 1283: `\errhelp` as the help, if there is one)
            let h = match self.get(&Key::TokPar(crate::val::TokPar::Errhelp)) {
                Val::Toks(t) if !t.is_empty() => Some(t),
                _ => None,
            };
            match h {
                Some(t) => self.tex_error_help(&s, super::interact::Help::ErrHelp(t), false),
                None => self.error(
                    &s,
                    &[
                        r"This error message was generated by an \errmessage",
                        r"command, so I can't give any explicit help.",
                        r"Pretend that you're Hercule Poirot: Examine all clues,",
                        r"and deduce the truth by order and method.",
                    ],
                ),
            }
        } else {
            // (tex.web 1280: with the selector in force; the `\newlinechar`
            // ends a line)
            let nl = self.int_par(IntPar::Newlinechar);
            let cur = self.selector();
            // (a plain entry prints to the log only)
            if cur == crate::print::LOG_ONLY && !s.chars().any(|c| i64::from(u32::from(c)) == i64::from(nl)) {
                self.log_push(&s);
                return;
            }
            let mut e = String::from(crate::show::RAW);
            e.push(crate::print::sel(cur));
            e.push(crate::show::MSG);
            e.extend(s.chars().map(|c| if i64::from(u32::from(c)) == i64::from(nl) { crate::show::LN } else { c }));
            e.push(crate::show::MSG_END);
            self.log_push(&e);
        }
    }

    /// `\uppercase`, `\lowercase` (tex.web 1285): the text with its
    /// characters' codes changed rewrites the input.
    pub(super) fn case_shift(&mut self, upper: bool) {
        let mut l = self.scan_toks_raw();
        for t in &mut l {
            let (c, active) = match t {
                Tok::Char(_, c) => (*c, false),
                Tok::Active(c) => (*c, true),
                _ => continue,
            };
            if u32::from(c) > 255 {
                continue;
            }
            let key = if upper { Key::Uccode(c) } else { Key::Lccode(c) };
            let v = self.int(&key);
            if v != 0 {
                let n = char::from_u32(v as u32).unwrap_or(c);
                *t = if active {
                    Tok::Active(n)
                } else {
                    let Tok::Char(cat, _) = t else { unreachable!() };
                    Tok::Char(*cat, n)
                };
            }
        }
        self.rewrite(l);
    }
}

/// `g + r`, the higher order of infinity winning (tex.web 1239).
fn add_glue(mut q: Glue, r: Glue) -> Glue {
    q.width = q.width.wrapping_add(r.width);
    if q.stretch == 0 {
        q.stretch_order = 0;
    }
    if q.stretch_order == r.stretch_order {
        q.stretch = q.stretch.wrapping_add(r.stretch);
    } else if q.stretch_order < r.stretch_order && r.stretch != 0 {
        q.stretch = r.stretch;
        q.stretch_order = r.stretch_order;
    }
    if q.shrink == 0 {
        q.shrink_order = 0;
    }
    if q.shrink_order == r.shrink_order {
        q.shrink = q.shrink.wrapping_add(r.shrink);
    } else if q.shrink_order < r.shrink_order && r.shrink != 0 {
        q.shrink = r.shrink;
        q.shrink_order = r.shrink_order;
    }
    q
}
