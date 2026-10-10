//! Material (tex.web parts 46–47, copied): commands as nodes that emit
//! items. A run of characters is one node (TeX's main loop, §1034–§1040:
//! ligatures and kerns from the font's lig/kern program, `\sfcode`s read
//! per character); spaces (§1041–§1044); glue, kerns, penalties, rules,
//! marks, removals (an `Item::Remove` the list's gatherer applies);
//! paragraphs (§1091, §1096); boxes (§1076–§1087: a box group's `}` is a
//! `Close` node defining nothing, which hands what it closed to the next
//! node, the packaging node — it gathers the contents from the items the
//! nodes inside emitted: nothing accumulates in a list structure);
//! `\shipout`; `\end` (§1054). Where TeX calls the page builder, the node
//! emits `Item::BuildPage` with what the page builder reads there.

use std::sync::Arc;

use super::{Chain, Exec, Group, GroupKind, Kind, Mode};
use crate::arith::xn_over_d;
use crate::item::{
    BoxNode, Disc, Interline, Item, Lig, Origins, PageCall, Para, RUNNING, ShipParams, Shipped, is_zero, param_glue,
};
use crate::linebreak::{self, LineParams};
use crate::pack::{Params, Spec, hpack, vpack};
use crate::page::{IGNORE_DEPTH, MAIN_DEPTH, page_mark, push_vertical, remove_last};
use crate::show::{Ctx, NL, RAW, report};
use crate::tfm::{Font, Glyph, LigKern, Tag};
use crate::val::{DimenPar, Glue, GluePar, IntPar, Key, Mat, Meaning, Prim, Tok, TokPar, Val};

/// What a box group is (tex.web 1083, 1117).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum BoxKind {
    H,
    V,
    Vtop,
    /// A discretionary's part (0 pre-break, 1 post-break, 2 replacement)
    /// and the parts before it.
    Disc(u8, Arc<(Vec<Item>, Vec<Item>)>),
    /// `\insert n` (255: `\vadjust`), with what its `}` read inside it
    /// (tex.web 1100).
    Insert(u8, Option<Arc<InsParams>>),
    /// `\vcenter` (tex.web 1167).
    Vcenter,
    /// A math group (`math.rs`).
    Math(Arc<super::math::MathKind>),
}

/// What an insertion reads as its group ends (tex.web 1100):
/// `\splittopskip`, `\splitmaxdepth`, `\floatingpenalty`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct InsParams {
    pub(super) split_top: Arc<Glue>,
    pub(super) depth: i32,
    pub(super) float: i32,
}

/// What happens to a box when it is made (tex.web 1075: `box_context`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BoxContext {
    /// Appended to the current list, shifted.
    Shift(i32),
    /// `\setbox n`, `\global` or not.
    Set(u16, bool),
    Ship,
    /// The box of `\leaders` (100), `\cleaders`, `\xleaders` (102).
    Leaders(u8),
}

/// An open box group: what it makes, where it goes, and the enclosing
/// list's state, restored at its end (TeX's `push_nest`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BoxGroup {
    pub(super) kind: BoxKind,
    pub(super) context: BoxContext,
    pub(super) spec: Spec,
    /// An `\hbox` appended in vertical mode: its marks move out (§1085).
    pub(super) adjusted: bool,
    pub(super) mode: Mode,
    pub(super) inner: bool,
    pub(super) space_factor: i32,
    pub(super) prev_depth: i32,
    /// The enclosing paragraph's language state.
    pub(super) hyph_lang: [u8; 4],
    /// The enclosing vertical list's `\prevgraf` (a new list's is 0).
    pub(super) prev_graf: i32,
}

/// A box group just closed, for the packaging node: the group, and
/// `\boxmaxdepth` as it was inside (§1086).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BoxEnd {
    pub(super) g: Arc<BoxGroup>,
    pub(super) boxmaxdepth: i32,
    /// What a math group's close read inside it.
    pub(super) math: Option<Arc<super::math::MathEnd>>,
}

/// tex.web 549: `non_char`.
const NON_CHAR: i32 = 256;

/// §1034: an entry of `lig_stack`.
#[derive(Clone, Copy)]
enum LigEntry {
    Char(u8),
    Item { ch: i32, ptr: Option<u8> },
}

impl LigEntry {
    fn ch(self) -> i32 {
        match self {
            LigEntry::Char(c) => i32::from(c),
            LigEntry::Item { ch, .. } => ch,
        }
    }
}

/// §1032: the main loop's registers.
struct Main {
    font: Arc<Font>,
    i: Glyph,
    j: LigKern,
    k: usize,
    bchar: i32,
    false_bchar: i32,
    cur_l: i32,
    cur_r: i32,
    cur_chr: i32,
    /// How many characters follow `cur_q` (they end the list).
    after_q: usize,
    lig_stack: Vec<LigEntry>,
    ligature_present: bool,
    lft_hit: bool,
    rt_hit: bool,
    ins_disc: bool,
    /// The token after the characters, read and not one.
    look: Option<Tok>,
    /// pdfTeX: the tags `\pdfnoligatures` and `\tagcode` cleared of the
    /// font's characters (`micro::TAG`'s table): one whose lig/kern
    /// program's is cleared has none.
    tags: Arc<[i32]>,
    /// pdfTeX's `\pdfprependkern`/`\pdfappendkern` kerns, if either is
    /// positive.
    auto: Option<AutoKern>,
}

/// What `get_auto_kern` reads of the main loop's font.
struct AutoKern {
    pre: bool,
    app: bool,
    quad: i32,
    knbc: Arc<[i32]>,
    knac: Arc<[i32]>,
}

impl AutoKern {
    /// pdftex.web's `get_auto_kern(f, l, r)`: the kern after `l` and
    /// before `r` (either `NON_CHAR`), if any.
    fn kern(&self, l: i32, r: i32) -> Option<Item> {
        let mut w = 0;
        if self.app && l < NON_CHAR {
            let k = crate::micro::code(&self.knac, crate::micro::KNAC, l as u8);
            if k != 0 {
                w = Font::round_xn_over_d(self.quad, k, 1000);
            }
        }
        if self.pre && r < NON_CHAR {
            let k = crate::micro::code(&self.knbc, crate::micro::KNBC, r as u8);
            if k != 0 {
                w += Font::round_xn_over_d(self.quad, k, 1000);
            }
        }
        (w != 0).then_some(Item::Kern { w, sub: 3 })
    }
}

impl Main {
    /// The lig/kern instruction at `k` (a stop for a bad index).
    fn lk(&self) -> LigKern {
        self.font.lig_kerns.get(self.k).copied().unwrap_or(LigKern {
            skip: LigKern::STOP,
            next: 0,
            op: 0,
            remainder: 0,
        })
    }
}

/// The main loop's labels.
#[derive(Clone, Copy)]
enum L {
    Wrapup,
    Move,
    Move1,
    Move2,
    MoveLig,
    Lookahead,
    Lookahead1,
    LigLoop,
    LigLoop1,
    LigLoop2,
    Reswitch,
    BigSwitch,
}

/// The first node of the paragraph at `level` that node `i` is in (or
/// would be): back to the node that began it.
pub(super) fn para_start(chain: &Chain, i: usize, level: u16) -> usize {
    let mut j = i;
    while j > 0 {
        let o = &chain.nodes[j - 1].out;
        if o.level > level || (o.level == level && o.mode == Mode::H) {
            j -= 1;
        } else {
            break;
        }
    }
    j
}

/// The first node inside the box at `level` that node `i` is in.
pub(super) fn box_start(chain: &Chain, i: usize, level: u16) -> usize {
    let mut j = i;
    while j > 0 && chain.nodes[j - 1].out.level >= level {
        j -= 1;
    }
    j
}

/// Appends an item emitted into a horizontal list, or applies it.
pub(super) fn push_horizontal(list: &mut Vec<Item>, it: Item) {
    match it {
        Item::Remove(k) => remove_last(list, k),
        _ => list.push(it),
    }
}

/// The fonts of a paragraph's characters, where its lines' expansion and
/// protrusion look (the list, its discretionaries, its hboxes).
fn para_fonts(list: &[Item], out: &mut Vec<Arc<Font>>) {
    for it in list {
        let f = match it {
            Item::Char(f, _) => f,
            Item::Lig(l) => &l.font,
            Item::Box(b) if !b.vertical => {
                para_fonts(&b.list, out);
                continue;
            }
            Item::Disc(d) => {
                para_fonts(&d.pre, out);
                para_fonts(&d.post, out);
                continue;
            }
            _ => continue,
        };
        if !out.iter().any(|x| Arc::ptr_eq(x, f) || x.key == f.key) {
            out.push(f.clone());
        }
    }
}

/// The fonts appearing in a list (for the names its display shows).
pub(super) fn fonts_in(list: &[Item], out: &mut Vec<Arc<str>>) {
    for it in list {
        let id = match it {
            Item::Char(f, _) => f.ident_key(),
            Item::Lig(l) => l.font.ident_key(),
            Item::Box(b) => {
                fonts_in(&b.list, out);
                continue;
            }
            Item::Disc(d) => {
                fonts_in(&d.pre, out);
                fonts_in(&d.post, out);
                continue;
            }
            Item::Leaders(l) => {
                fonts_in(std::slice::from_ref(&l.b), out);
                continue;
            }
            Item::Ins(i) => {
                fonts_in(&i.list, out);
                continue;
            }
            Item::Adjust(a) => {
                fonts_in(a, out);
                continue;
            }
            Item::Noad(m) => {
                let field = |f: &crate::mlist::Field, out: &mut Vec<Arc<str>>| match f {
                    crate::mlist::Field::Box(b) => fonts_in(std::slice::from_ref(&Item::Box(b.clone())), out),
                    crate::mlist::Field::Mlist(l) => fonts_in(l, out),
                    _ => {}
                };
                match &**m {
                    crate::mlist::MItem::Noad(n) => {
                        field(&n.nucleus, out);
                        field(&n.sup, out);
                        field(&n.sub, out);
                    }
                    crate::mlist::MItem::Choice(c) => {
                        for l in &**c {
                            fonts_in(l, out);
                        }
                    }
                    _ => {}
                }
                continue;
            }
            _ => continue,
        };
        if !out.iter().any(|x| **x == **id) {
            out.push(id.clone());
        }
    }
}

impl Exec<'_> {
    // ------------------------------------------------------------ reads

    pub(super) fn glue_par(&mut self, p: GluePar) -> Arc<Glue> {
        match self.get(&Key::GluePar(p)) {
            Val::Glue(g) => g,
            _ => Arc::new(Glue::default()),
        }
    }

    pub(super) fn toks_par(&mut self, p: TokPar) -> Vec<Tok> {
        match self.get(&Key::TokPar(p)) {
            Val::Toks(t) => t.to_vec(),
            _ => Vec::new(),
        }
    }

    pub(super) fn interline(&mut self) -> Arc<Interline> {
        Arc::new(Interline {
            baselineskip: self.glue_par(GluePar::Baselineskip),
            lineskip: self.glue_par(GluePar::Lineskip),
            lineskiplimit: self.dimen_par(DimenPar::Lineskiplimit),
        })
    }

    /// What shipping a page reads (tex.web 638–641, 617).
    pub(super) fn ship_params(&mut self) -> Arc<ShipParams> {
        let mut counts = [0; 10];
        for (k, c) in counts.iter_mut().enumerate() {
            *c = self.int(&Key::Count(k as u16));
        }
        let pdf = crate::pdf::read_ship(&mut ExecShip(self));
        Arc::new(ShipParams {
            counts,
            hoffset: self.dimen_par(DimenPar::Hoffset),
            voffset: self.dimen_par(DimenPar::Voffset),
            mag: self.int_par(IntPar::Mag),
            date: [
                self.int_par(IntPar::Year),
                self.int_par(IntPar::Month),
                self.int_par(IntPar::Day),
                self.int_par(IntPar::Time),
            ],
            pdf,
        })
    }

    /// What the page builder reads where it is called.
    pub(super) fn page_call(&mut self) -> Arc<PageCall> {
        Arc::new(PageCall {
            vsize: self.dimen_par(DimenPar::Vsize),
            maxdepth: self.dimen_par(DimenPar::Maxdepth),
            topskip: self.glue_par(GluePar::Topskip),
            hsize: self.dimen_par(DimenPar::Hsize),
        })
    }

    /// What a display of `list` reads: the escape character, the names of
    /// its fonts, `\showboxdepth` and `\showboxbreadth`.
    pub(super) fn show_ctx(&mut self, list: &[Item]) -> Ctx {
        let mut ids = Vec::new();
        fonts_in(list, &mut ids);
        let idents = ids
            .into_iter()
            .map(|id| {
                let n = match self.get(&Key::FontIdent(id.clone())) {
                    Val::Name(n) => n,
                    _ => Arc::from("FONT"),
                };
                (id, n)
            })
            .collect();
        Ctx {
            esc: self.int_par(IntPar::Escapechar),
            idents,
            depth: self.int_par(IntPar::Showboxdepth),
            breadth: self.int_par(IntPar::Showboxbreadth),
            newline: self.int_par(IntPar::Newlinechar),
            cs: self.env.cs().clone(),
            max_print_line: self.env.limits().max_print_line as i32,
        }
    }

    /// Appends a raw entry to the node's log: to its last entry if that is
    /// raw, else as an entry of its own (`logs::SEP`).
    pub(super) fn log_raw(&mut self, raw: &str) {
        match &mut self.log {
            Some(l) if l.rsplit(crate::logs::SEP).next().is_some_and(|e| e.starts_with(RAW)) => {
                let mut s = l.to_string();
                s.push_str(raw.strip_prefix(RAW).unwrap_or(raw));
                *l = s.into_boxed_str();
            }
            Some(_) => self.log_push(raw),
            None => self.log = Some(Box::from(raw)),
        }
    }

    /// Emits into the current list (vertical or horizontal).
    pub(super) fn emit_here(&mut self, it: Item) {
        if self.st.mode == Mode::V {
            self.vemit.push(it);
        } else {
            self.emit.push(it);
        }
    }

    /// The page builder is called here (only on the main vertical list,
    /// tex.web 994).
    pub(super) fn build_page(&mut self) {
        if self.st.level == 0 && self.st.mode == Mode::V && !self.st.in_output() {
            let c = self.page_call();
            self.vemit.push(Item::BuildPage(c));
        }
    }

    // ------------------------------------------------------------ dispatch

    /// A material command, by mode (tex.web 1045).
    pub(super) fn mat(&mut self, x: Mat, t: &Tok) {
        let v = self.st.mode == Mode::V;
        match x {
            Mat::Hskip(k) => {
                if v {
                    self.back_new_graf(t);
                } else {
                    self.append_glue(k);
                }
            }
            Mat::Vskip(k) => {
                if v {
                    self.append_glue(k);
                } else {
                    self.head_for_vmode(t, false);
                }
            }
            Mat::Kern => {
                let w = self.scan_dimen();
                self.emit_here(Item::Kern { w, sub: 1 });
            }
            Mat::Penalty => {
                let p = self.scan_int();
                self.emit_here(Item::Penalty(p));
                self.build_page();
            }
            Mat::Hrule => {
                if v {
                    self.rule(true);
                } else {
                    self.head_for_vmode(t, true);
                }
            }
            Mat::Vrule => {
                if v {
                    self.back_new_graf(t);
                } else {
                    self.rule(false);
                }
            }
            Mat::Remove(k) => self.emit_here(Item::Remove(k)),
            Mat::Mark => {
                let toks = self.scan_toks_x();
                self.emit_here(Item::Mark(Arc::new(toks)));
            }
            Mat::MakeBox(k) => self.begin_box(k, BoxContext::Shift(0)),
            Mat::SetBox => self.setbox(false),
            Mat::Unhbox(copy) => {
                if v {
                    self.back_new_graf(t);
                } else {
                    self.unpackage(copy, false);
                }
            }
            Mat::Unvbox(copy) => {
                if v {
                    self.unpackage(copy, true);
                } else {
                    self.head_for_vmode(t, false);
                }
            }
            Mat::Vmove(lower) => {
                if v {
                    self.err(format!("You can't use `{}' in vertical mode", self.shown(t)));
                } else {
                    let d = self.scan_dimen();
                    self.scan_box(BoxContext::Shift(if lower { d } else { -d }));
                }
            }
            Mat::Hmove(left) => {
                if v {
                    let d = self.scan_dimen();
                    self.scan_box(BoxContext::Shift(if left { -d } else { d }));
                } else {
                    self.err(format!("You can't use `{}' in horizontal mode", self.shown(t)));
                }
            }
            Mat::Shipout => self.scan_box(BoxContext::Ship),
            Mat::Disc(explicit) => {
                if v {
                    self.back_new_graf(t);
                } else {
                    self.append_discretionary(explicit);
                }
            }
            Mat::ItalCorr => {
                if v {
                    self.err(format!("You can't use `{}' in vertical mode", self.shown(t)));
                } else {
                    self.italic_correction();
                }
            }
            Mat::FontInt(k) => self.assign_font_int(k),
            Mat::Leaders(sub) => self.scan_box(BoxContext::Leaders(sub)),
            Mat::Accent => {
                if v {
                    self.back_new_graf(t);
                } else {
                    self.make_accent();
                }
            }
            Mat::Insert => self.begin_insert(false),
            Mat::Vadjust => {
                if v {
                    self.err(format!("You can't use `{}' in vertical mode", self.shown(t)));
                } else {
                    self.begin_insert(true);
                }
            }
            Mat::Internal(0) => self.package(),
            Mat::Internal(1) => self.end_par(),
            Mat::Internal(2) => self.disc_next(),
            Mat::Internal(3) => self.normal_paragraph(),
            Mat::Internal(4) => self.math_init(),
            Mat::Internal(5) => self.display_open(),
            Mat::Internal(6) => self.display_end(),
            Mat::Internal(7) => self.math_group_open(),
            Mat::Internal(9) => self.middle_next(),
            Mat::Internal(10) => self.output_begin(),
            Mat::Internal(11) => self.output_page_state(),
            Mat::Internal(_) => self.choice_next(),
        }
    }

    /// vmode + a horizontal command: a paragraph begins first (tex.web
    /// 1090).
    pub(super) fn back_new_graf(&mut self, t: &Tok) {
        self.back(t.clone());
        self.new_graf(true);
    }

    /// tex.web 1095: `head_for_vmode`.
    pub(super) fn head_for_vmode(&mut self, t: &Tok, hrule: bool) {
        if self.st.inner {
            if hrule {
                self.err("You can't use `\\hrule' here except with leaders".into());
            } else {
                // (`off_save`: the box's `}` goes in first)
                self.err("Missing } inserted".into());
                self.rewrite(vec![Tok::Char(2, '}'), t.clone()]);
            }
        } else {
            let par = self.cs("par");
            self.rewrite(vec![par, t.clone()]);
        }
    }

    // ------------------------------------------------------------ paragraphs

    /// tex.web 1091: `new_graf`.
    pub(super) fn new_graf(&mut self, indented: bool) {
        self.st.prev_graf = 0;
        let outer = self.st.level == 0 && !self.st.in_output();
        if outer || !self.current_list(true).is_empty() {
            let g = self.glue_par(GluePar::Parskip);
            self.vemit.push(param_glue(&g, GluePar::Parskip as u8));
        }
        self.st.mode = Mode::H;
        self.st.inner = false;
        self.st.space_factor = 1000;
        self.begin_paragraph_language();
        if indented {
            let w = self.dimen_par(DimenPar::Parindent);
            self.emit.push(Item::Box(Arc::new(BoxNode { w, ..BoxNode::default() })));
        }
        let ep = self.toks_par(TokPar::Everypar);
        self.rewrite(ep);
        if outer {
            let c = self.page_call();
            self.vemit.push(Item::BuildPage(c));
        }
    }

    /// `\indent`, `\noindent` (tex.web 1090, 1092).
    pub(super) fn start_par(&mut self, indent: bool) {
        if self.st.mode == Mode::V {
            self.new_graf(indent);
        } else if indent {
            let w = self.dimen_par(DimenPar::Parindent);
            self.emit.push(Item::Box(Arc::new(BoxNode { w, ..BoxNode::default() })));
            self.st.space_factor = 1000;
        }
    }

    /// tex.web 1070: `normal_paragraph`.
    pub(super) fn normal_paragraph(&mut self) {
        if self.int_par(IntPar::Looseness) != 0 {
            self.define(Key::IntPar(IntPar::Looseness), Val::Int(0), false);
        }
        if self.dimen_par(DimenPar::Hangindent) != 0 {
            self.define(Key::DimenPar(DimenPar::Hangindent), Val::Int(0), false);
        }
        if self.int_par(IntPar::Hangafter) != 1 {
            self.define(Key::IntPar(IntPar::Hangafter), Val::Int(1), false);
        }
        if matches!(self.get(&Key::Parshape), Val::Parshape(s) if !s.is_empty()) {
            self.define(Key::Parshape, Val::Parshape(Arc::new(Vec::new())), false);
        }
        // (etex.web §1070: and `\interlinepenalties`)
        if !self.penalties(0).is_empty() {
            self.define(Key::PenaltiesArray(0), Val::Penalties(Arc::from([])), false);
        }
    }

    /// Whether `normal_paragraph` would define anything.
    pub(super) fn paragraph_shaped(&mut self) -> bool {
        self.int_par(IntPar::Looseness) != 0
            || self.dimen_par(DimenPar::Hangindent) != 0
            || self.int_par(IntPar::Hangafter) != 1
            || matches!(self.get(&Key::Parshape), Val::Parshape(s) if !s.is_empty())
            || !self.penalties(0).is_empty()
    }

    /// `\par` (tex.web 1094).
    pub(super) fn par_cmd(&mut self) {
        if self.st.mode == Mode::V {
            self.normal_paragraph();
            self.build_page();
        } else if self.st.al.state < 0 {
            // (tex.web 1094: an alignment entry's braces are unbalanced:
            // `off_save`)
            let t = self.cmd.clone();
            self.math_off_save(&t);
        } else if !self.st.inner {
            self.end_par();
            self.build_page();
        }
    }

    /// What line breaking reads (tex.web 813–890).
    pub(super) fn line_params(&mut self, hlist: &[Item]) -> LineParams {
        let parshape = match self.get(&Key::Parshape) {
            Val::Parshape(s) => s,
            _ => Arc::new(Vec::new()),
        };
        LineParams {
            hsize: self.dimen_par(DimenPar::Hsize),
            pretolerance: self.int_par(IntPar::Pretolerance),
            tolerance: self.int_par(IntPar::Tolerance),
            emergency_stretch: self.dimen_par(DimenPar::Emergencystretch),
            looseness: self.int_par(IntPar::Looseness),
            linepenalty: self.int_par(IntPar::Linepenalty),
            hyphenpenalty: self.int_par(IntPar::Hyphenpenalty),
            exhyphenpenalty: self.int_par(IntPar::Exhyphenpenalty),
            adjdemerits: self.int_par(IntPar::Adjdemerits),
            doublehyphendemerits: self.int_par(IntPar::Doublehyphendemerits),
            finalhyphendemerits: self.int_par(IntPar::Finalhyphendemerits),
            interlinepenalty: self.int_par(IntPar::Interlinepenalty),
            clubpenalty: self.int_par(IntPar::Clubpenalty),
            widowpenalty: self.int_par(IntPar::Widowpenalty),
            brokenpenalty: self.int_par(IntPar::Brokenpenalty),
            interlinepenalties: self.penalties(0),
            clubpenalties: self.penalties(1),
            widowpenalties: self.penalties(2),
            last_line_fit: self.int_par(IntPar::Lastlinefit),
            texxet: self.int_par(IntPar::TeXXeTstate) > 0,
            leftskip: self.glue_par(GluePar::Leftskip),
            rightskip: self.glue_par(GluePar::Rightskip),
            hangindent: self.dimen_par(DimenPar::Hangindent),
            hangafter: self.int_par(IntPar::Hangafter),
            parshape,
            prev_graf: self.st.prev_graf,
            pack: Params {
                badness: self.int_par(IntPar::Hbadness),
                fuzz: self.dimen_par(DimenPar::Hfuzz),
                overfull_rule: self.dimen_par(DimenPar::Overfullrule),
            },
            inter: self.interline(),
            show: self.show_ctx(hlist),
            hyph: Some(self.hyph_params(hlist)),
            micro: self.micro(hlist),
        }
    }

    /// What pdfTeX's font expansion and character protrusion read of the
    /// paragraph's fonts, if `\pdfadjustspacing` or `\pdfprotrudechars`
    /// is positive (pdftex.web: `check_expand_pars` and the codes
    /// `char_stretch`, `char_pw` read).
    fn micro(&mut self, hlist: &[Item]) -> Option<Arc<crate::micro::Micro>> {
        use crate::micro::{Expand, Micro, MicroFont};
        let adjust = self.int_par(IntPar::Pdfadjustspacing);
        let protrude = self.int_par(IntPar::Pdfprotrudechars);
        if adjust <= 0 && protrude <= 0 {
            return None;
        }
        let mut fonts: Vec<Arc<Font>> = Vec::new();
        para_fonts(hlist, &mut fonts);
        let mut out = Micro { adjust_spacing: adjust, protrude_chars: protrude, fonts: Vec::new(), max_stretch: -1, max_shrink: -1, step: -1 };
        let empty: Arc<[i32]> = Arc::from([]);
        for f in fonts {
            let k = self.font_key(&f);
            let (ef, expand) = if adjust > 0 {
                let x = self.font_expand_of(&f);
                (self.font_code_table(&Key::FontCode(crate::micro::EF, k.clone())), x)
            } else {
                (empty.clone(), None)
            };
            let (lp, rp, quad) = if protrude > 0 {
                (
                    self.font_code_table(&Key::FontCode(crate::micro::LP, k.clone())),
                    self.font_code_table(&Key::FontCode(crate::micro::RP, k.clone())),
                    self.int(&Key::FontDimen(k, 6)),
                )
            } else {
                (empty.clone(), empty.clone(), 0)
            };
            if let Some(e) = expand.filter(Expand::expandable) {
                // (`check_expand_pars`: one step and one pair of limits
                // in a paragraph)
                let bad = if out.step < 0 {
                    out.step = e.step;
                    None
                } else if out.step != e.step {
                    Some("using fonts with different step of expansion in one paragraph is not allowed")
                } else {
                    None
                };
                let mut bad = bad;
                if e.stretch > 0 {
                    if out.max_stretch < 0 {
                        out.max_stretch = e.stretch;
                    } else if out.max_stretch != e.stretch {
                        bad = Some("using fonts with different limit of expansion in one paragraph is not allowed");
                    }
                }
                if e.shrink > 0 {
                    if out.max_shrink < 0 {
                        out.max_shrink = e.shrink;
                    } else if out.max_shrink != e.shrink {
                        bad = Some("using fonts with different limit of expansion in one paragraph is not allowed");
                    }
                }
                if let Some(b) = bad
                    && adjust > 1
                {
                    self.pdf_error("font expansion", b);
                }
            }
            out.fonts.push(MicroFont { font: f, quad, ef, lp, rp, expand });
        }
        Some(Arc::new(out))
    }

    /// `\par` in horizontal mode (tex.web 1096: `end_graf`): the
    /// paragraph, from the items its nodes emitted. On the main vertical
    /// list its lines are a separate task's (`Para`); inside a box they
    /// are broken here.
    pub(super) fn end_par(&mut self) {
        if self.st.mode != Mode::H || self.st.inner {
            return;
        }
        self.kind = Kind::Par;
        let level = self.st.level;
        let nodes = &self.chain.nodes;
        let start = para_start(self.chain, nodes.len(), level);
        let me = nodes.len();
        let mut hlist = Vec::new();
        // (which node made each item: its distance back from this one)
        let mut orig: Vec<u32> = Vec::new();
        for (j, n) in nodes.range(start, nodes.len()).enumerate() {
            if n.out.level == level && n.out.mode == Mode::H {
                for it in n.emit.iter() {
                    push_horizontal(&mut hlist, it.clone());
                    orig.resize(hlist.len(), (me - start - j) as u32);
                }
            }
        }
        for it in std::mem::take(&mut self.emit) {
            push_horizontal(&mut hlist, it);
            orig.resize(hlist.len(), 0);
        }
        // (a `\shipout` inside the paragraph ships before its lines come)
        let ships: Vec<Item> = hlist.iter().filter(|i| matches!(i, Item::Shipout(_))).cloned().collect();
        let mut keep = hlist.iter().map(|i| !matches!(i, Item::Shipout(_)));
        orig.retain(|_| keep.next().unwrap_or(true));
        hlist.retain(|i| !matches!(i, Item::Shipout(_)));
        self.st.mode = Mode::V;
        self.st.inner = level > 0;
        let mut para = Para { before: ships, ..Para::default() };
        if !hlist.is_empty() {
            // (§816)
            if hlist.last().is_some_and(Item::is_glue) {
                hlist.pop();
            }
            hlist.push(Item::Penalty(10000));
            let g = self.glue_par(GluePar::Parfillskip);
            hlist.push(param_glue(&g, GluePar::Parfillskip as u8));
            orig.resize(hlist.len(), 0);
            let origins = Origins::from_runs(orig.iter().enumerate().map(|(i, &d)| (i as u32, d)));
            let lp = self.line_params(&hlist);
            if level == 0 {
                hlist.shrink_to_fit();
                para.hlist = hlist;
                para.origins = origins;
                para.lb = Some(Arc::new(lp));
                // (its last line's depth, once broken: `Exec::prev_depth`)
                self.st.prev_depth = MAIN_DEPTH;
            } else {
                let lines = linebreak::lines(&hlist, &lp);
                self.vemit.append(&mut para.before);
                for l in &lines {
                    if let Some(r) = &l.report {
                        self.log_raw(r);
                    }
                    // (inside a box: the line's items by the nodes that made
                    // them, back from this one)
                    let mut b = (*l.b).clone();
                    b.origins = Origins::from_runs(
                        (0..b.list.len()).filter_map(|i| Some((i as u32, origins.of(l.list_item(i)?)?))),
                    );
                    self.vemit.push(Item::Append(Arc::new(b), lp.inter.clone()));
                    self.vemit.extend(l.adjust.iter().cloned());
                    if let Some(x) = l.pen {
                        self.vemit.push(Item::Penalty(x));
                    }
                }
                self.st.prev_graf = linebreak::prev_graf_after(&lp, lines.len());
                if let Some(l) = lines.last() {
                    self.st.prev_depth = l.b.d;
                }
            }
        }
        if level == 0 && (!para.before.is_empty() || para.lb.is_some()) {
            // (equal to the paragraph the old node here made: that one)
            self.para = Some(match self.old_para.take() {
                Some(o) if *o == para && o.origins.0 == para.origins.0 => o,
                _ => Arc::new(para),
            });
        }
        self.normal_paragraph();
        // (§1096)
        if !self.halted() {
            self.st.error_count = 0;
        }
    }

    // ------------------------------------------------------------ the current list

    /// The current list's items so far (vertical or horizontal), with the
    /// gatherer's rules applied — the one read of material (CORE.md): a
    /// node reading it is run again whenever its paragraph's nodes are.
    pub(super) fn current_list(&mut self, vertical: bool) -> Vec<Item> {
        let level = self.st.level;
        let nodes = &self.chain.nodes;
        let mut list = Vec::new();
        if vertical && level == 0 {
            // (the main vertical list: what this piece contributed since its
            // last paragraph's lines, which are a separate task's)
            let mut from = 0;
            for (j, n) in nodes.iter().enumerate() {
                if n.para().is_some_and(|p| p.lb.is_some()) {
                    from = j + 1;
                }
            }
            for n in nodes.range(from, nodes.len()) {
                if let Some(p) = n.para() {
                    for it in p.before.iter().chain(&p.after) {
                        list.push(it.clone());
                    }
                }
            }
            let mut pd = IGNORE_DEPTH;
            let mut out = Vec::new();
            for it in list.into_iter().chain(self.vemit.iter().cloned()) {
                push_vertical(&mut out, &mut pd, it);
            }
            return out;
        }
        if vertical {
            let start = box_start(self.chain, nodes.len(), level);
            let mut pd = IGNORE_DEPTH;
            for n in nodes.range(start, nodes.len()) {
                if n.out.level == level {
                    for it in n.vemit() {
                        push_vertical(&mut list, &mut pd, it.clone());
                    }
                }
            }
            for it in self.vemit.clone() {
                push_vertical(&mut list, &mut pd, it);
            }
            return list;
        }
        let start = if self.st.inner || self.st.mode == Mode::M {
            box_start(self.chain, nodes.len(), level)
        } else {
            para_start(self.chain, nodes.len(), level)
        };
        for n in nodes.range(start, nodes.len()) {
            if n.out.level == level && n.out.mode == Mode::H {
                for it in n.emit.iter() {
                    push_horizontal(&mut list, it.clone());
                }
            }
        }
        for it in self.emit.clone() {
            push_horizontal(&mut list, it);
        }
        list
    }

    /// The current list's last item (`\lastskip`, `\/`: tex.web 424,
    /// 1113).
    pub(super) fn last_item(&mut self) -> Option<Item> {
        if self.kind == Kind::Other {
            self.kind = Kind::Par;
        }
        let v = self.st.mode == Mode::V;
        let list = self.current_list(v);
        let last = list.into_iter().rev().find(|i| !matches!(i, Item::BuildPage(_) | Item::End(_) | Item::Shipout(_)));
        if last.is_some() || !v || self.st.level != 0 || self.st.in_output() {
            return last;
        }
        // (the main vertical list, nothing contributed since its last
        // paragraph or in this piece: that paragraph's tail, or the one
        // reaching the piece, tex.web 424)
        let nodes = &self.chain.nodes;
        let lb = (0..nodes.len()).rev().find(|&j| nodes[j].para().is_some_and(|p| p.lb.is_some()));
        if let Some(j) = lb {
            return nodes[j].para().and_then(|p| match main_tail(p) {
                Some(Val::Tail(t)) => Some((*t).clone()),
                _ => None,
            });
        }
        match self.get(&Key::MainTail) {
            Val::Tail(t) => Some((*t).clone()),
            _ => None,
        }
    }

    /// `\prevdepth` of the current vertical list; the main vertical
    /// list's where the reader state says `page::MAIN_DEPTH` (a piece's
    /// start, after a paragraph not broken into lines yet): a read of
    /// `Key::MainDepth`, a paragraph's its last line's depth.
    pub(super) fn prev_depth(&mut self) -> i32 {
        if self.st.prev_depth != MAIN_DEPTH {
            return self.st.prev_depth;
        }
        self.main_depth()
    }

    /// The main vertical list's `\prevdepth` where a reader state says
    /// `page::MAIN_DEPTH`: `Key::MainDepth`.
    pub(super) fn main_depth(&mut self) -> i32 {
        match self.get(&Key::MainDepth) {
            Val::Int(d) => d,
            Val::Para(p) => match &p.lb {
                Some(lp) => linebreak::lines(&p.hlist, lp).last().map_or(IGNORE_DEPTH, |l| l.b.d),
                None => IGNORE_DEPTH,
            },
            _ => IGNORE_DEPTH,
        }
    }

    // ------------------------------------------------------------ characters

    /// A character command (letter, other, `\chardef`'d): in vertical mode
    /// it begins a paragraph first.
    pub(super) fn char_cmd(&mut self, c: u32, t: &Tok) {
        if self.st.mode == Mode::V {
            self.back_new_graf(t);
        } else {
            self.main_loop(c);
        }
    }

    /// `\char`.
    pub(super) fn char_cmd_num(&mut self, t: &Tok) {
        if self.st.mode == Mode::V {
            self.back_new_graf(t);
        } else {
            let c = self.scan_char_num();
            self.main_loop(c as u32);
        }
    }

    /// tex.web 581: `char_warning`, with `\tracinglostchars` positive.
    pub(super) fn char_warning(&mut self, f: &Font, c: i32) {
        if self.int_par(IntPar::Tracinglostchars) > 0 {
            let ch = char::from_u32(c as u32).unwrap_or('?');
            let s = format!("{RAW}{NL}Missing character: There is no {ch} in font {}!{NL}", f.name);
            self.log_raw(&s);
        }
    }

    /// §1034: `adjust_space_factor`.
    fn adjust_space_factor(&mut self, c: i32) {
        let ch = char::from_u32(c as u32).unwrap_or('\0');
        let s = self.int(&Key::Sfcode(ch));
        if s == 1000 {
            self.st.space_factor = 1000;
        } else if s < 1000 {
            if s > 0 {
                self.st.space_factor = s;
            }
        } else if self.st.space_factor < 1000 {
            self.st.space_factor = 1000;
        } else {
            self.st.space_factor = s;
        }
    }

    /// §1034–§1040: the main loop, from character `c0`: the characters
    /// after it, with their ligatures and kerns, while the next token is a
    /// character; that token goes back.
    fn main_loop(&mut self, c0: u32) {
        let font = self.font();
        let c0 = c0.min(255) as i32;
        self.adjust_space_factor(c0);
        self.fix_language();
        let mut m = Main {
            bchar: font.bchar,
            false_bchar: font.false_bchar,
            font,
            i: Glyph::default(),
            j: LigKern { skip: LigKern::STOP + 1, next: 0, op: 0, remainder: 0 },
            k: 0,
            cur_l: c0,
            cur_r: 0,
            cur_chr: c0,
            after_q: 0,
            lig_stack: vec![LigEntry::Char(c0 as u8)],
            ligature_present: false,
            lft_hit: false,
            rt_hit: false,
            ins_disc: false,
            look: None,
            tags: Arc::from([]),
            auto: None,
        };
        let key = self.font_key(&m.font);
        m.tags = self.font_code_table(&Key::FontCode(crate::micro::TAG, key.clone()));
        m.auto = self.auto_kern(&key);
        let mut l = if let Some(k) = m.auto.as_ref().and_then(|a| a.kern(NON_CHAR, m.cur_l)) {
            // (pdftex.web: a kern before the first character, which then
            // has no left boundary)
            self.emit.push(k);
            L::Move
        } else {
            match m.font.bchar_label {
                None => L::Move2,
                Some(k) => {
                    m.k = usize::from(k);
                    m.cur_r = m.cur_l;
                    m.cur_l = NON_CHAR;
                    L::LigLoop1
                }
            }
        };
        loop {
            l = match l {
                L::Wrapup => {
                    let z = m.rt_hit;
                    self.wrapup(&mut m, z);
                    L::Move
                }
                L::Move => match m.lig_stack.last() {
                    None => L::Reswitch,
                    Some(top) => {
                        m.after_q = 0;
                        m.cur_l = top.ch();
                        L::Move1
                    }
                },
                L::Move1 => {
                    if let Some(LigEntry::Char(_)) = m.lig_stack.last() {
                        L::Move2
                    } else {
                        L::MoveLig
                    }
                }
                L::Move2 => {
                    let g = m.font.glyph(m.cur_l);
                    match g {
                        Some(g) if m.cur_chr >= m.font.bc && m.cur_chr <= m.font.ec => {
                            m.i = g;
                            if let Some(LigEntry::Char(c)) = m.lig_stack.pop() {
                                self.emit.push(Item::Char(m.font.clone(), c));
                                m.after_q += 1;
                            }
                            L::Lookahead
                        }
                        _ => {
                            self.char_warning(&m.font, m.cur_chr);
                            m.lig_stack.clear();
                            L::BigSwitch
                        }
                    }
                }
                L::MoveLig => {
                    // (§1037)
                    let ptr = match m.lig_stack.pop() {
                        Some(LigEntry::Item { ptr, .. }) => ptr,
                        _ => None,
                    };
                    if let Some(c) = ptr {
                        self.emit.push(Item::Char(m.font.clone(), c));
                        m.after_q += 1;
                    }
                    m.i = m.font.glyph(m.cur_l).unwrap_or_default();
                    m.ligature_present = true;
                    match m.lig_stack.last() {
                        None if ptr.is_none() => {
                            m.cur_r = m.bchar;
                            L::LigLoop
                        }
                        None => L::Lookahead,
                        Some(top) => {
                            m.cur_r = top.ch();
                            L::LigLoop
                        }
                    }
                }
                L::Lookahead => {
                    // (§1038)
                    let c = match self.next_x_m(false) {
                        Some((_, Meaning::Char(11 | 12, c))) if u32::from(c) < 256 => Some(u32::from(c) as i32),
                        Some((_, Meaning::CharGiven(c))) if c < 256 => Some(c as i32),
                        Some((_, Meaning::Prim(Prim::Char))) => Some(self.scan_char_num()),
                        Some((t, _)) => {
                            m.look = Some(t);
                            None
                        }
                        None => None,
                    };
                    if let Some(c) = c {
                        m.cur_chr = c;
                        L::Lookahead1
                    } else {
                        m.cur_r = m.bchar;
                        m.lig_stack.clear();
                        L::LigLoop
                    }
                }
                L::Lookahead1 => {
                    self.adjust_space_factor(m.cur_chr);
                    m.lig_stack.clear();
                    m.lig_stack.push(LigEntry::Char(m.cur_chr as u8));
                    m.cur_r = m.cur_chr;
                    if m.cur_r == m.false_bchar {
                        m.cur_r = NON_CHAR;
                    }
                    L::LigLoop
                }
                L::LigLoop => match m.i.tag {
                    // (pdftex.web: a `\pdfappendkern`/`\pdfprependkern`
                    // kern between the two instead of their program)
                    _ if let Some(k) = m.auto.as_ref().and_then(|a| a.kern(m.cur_l, m.cur_r)) => {
                        let z = m.rt_hit;
                        if self.wrapup(&mut m, z) {
                            let at = self.emit.len() - 1;
                            self.emit.insert(at, k);
                        } else {
                            self.emit.push(k);
                        }
                        L::Move
                    }
                    // (§1039)
                    Tag::Lig(start)
                        if m.cur_r != NON_CHAR
                            && m.tags.get(m.cur_l as usize).is_none_or(|t| t & crate::micro::TAG_LIG == 0) =>
                    {
                        m.k = usize::from(start);
                        m.j = m.lk();
                        if m.j.skip <= LigKern::STOP {
                            L::LigLoop2
                        } else {
                            m.k = 256 * usize::from(m.j.op) + usize::from(m.j.remainder);
                            L::LigLoop1
                        }
                    }
                    _ => L::Wrapup,
                },
                L::LigLoop1 => {
                    m.j = m.lk();
                    L::LigLoop2
                }
                L::LigLoop2 => {
                    if i32::from(m.j.next) == m.cur_r && m.j.skip <= LigKern::STOP {
                        self.lig_kern_command(&mut m)
                    } else if m.j.skip == 0 {
                        m.k += 1;
                        L::LigLoop1
                    } else if m.j.skip >= LigKern::STOP {
                        L::Wrapup
                    } else {
                        m.k += usize::from(m.j.skip) + 1;
                        L::LigLoop1
                    }
                }
                L::Reswitch => {
                    if let Some(t) = m.look.take() {
                        self.back(t);
                    }
                    return;
                }
                L::BigSwitch => return,
            };
        }
    }

    /// §1035: `pack_lig(z)`.
    fn pack_lig(&mut self, m: &mut Main, z: bool) {
        let n = m.after_q.min(self.emit.len());
        let orig: Vec<u8> = self
            .emit
            .drain(self.emit.len() - n..)
            .filter_map(|i| if let Item::Char(_, c) = i { Some(c) } else { None })
            .collect();
        let left = std::mem::take(&mut m.lft_hit);
        let mut right = false;
        if z && m.lig_stack.is_empty() {
            right = true;
            m.rt_hit = false;
        }
        // (pdftex.web: a `\pdfprependkern` kern before it)
        if let Some(k) = m.auto.as_ref().filter(|a| a.pre).and_then(|a| a.kern(NON_CHAR, m.cur_l)) {
            self.emit.push(k);
        }
        self.emit.push(Item::Lig(Arc::new(Lig { font: m.font.clone(), c: m.cur_l as u8, orig, left, right })));
        m.after_q = 0;
        m.ligature_present = false;
    }

    /// What `get_auto_kern` reads (`None`: neither `\pdfprependkern` nor
    /// `\pdfappendkern` is positive).
    fn auto_kern(&mut self, key: &Arc<str>) -> Option<AutoKern> {
        let pre = self.int_par(IntPar::Pdfprependkern) > 0;
        let app = self.int_par(IntPar::Pdfappendkern) > 0;
        if !pre && !app {
            return None;
        }
        let quad = self.int(&Key::FontDimen(key.clone(), 6));
        let knbc = if pre { self.font_code_table(&Key::FontCode(crate::micro::KNBC, key.clone())) } else { Arc::from([]) };
        let knac = if app { self.font_code_table(&Key::FontCode(crate::micro::KNAC, key.clone())) } else { Arc::from([]) };
        Some(AutoKern { pre, app, quad, knbc, knac })
    }

    /// §1035: `wrapup(z)`: whether it put in a null discretionary.
    fn wrapup(&mut self, m: &mut Main, z: bool) -> bool {
        if m.cur_l < NON_CHAR {
            if m.after_q > 0
                && let Some(Item::Char(_, c)) = self.emit.last()
            {
                let c = i32::from(*c);
                let k = self.font_key(&m.font);
                if c == self.int(&Key::HyphenChar(k)) {
                    m.ins_disc = true;
                }
            }
            if m.ligature_present {
                self.pack_lig(m, z);
            }
            if m.ins_disc {
                m.ins_disc = false;
                if !self.st.inner {
                    self.emit.push(Item::Disc(Arc::new(Disc { pre: Vec::new(), post: Vec::new(), replace: 0 })));
                    return true;
                }
            }
        }
        false
    }

    /// §1040: a ligature or kern instruction.
    fn lig_kern_command(&mut self, m: &mut Main) -> L {
        let op = m.j.op;
        if op >= LigKern::KERN {
            let z = m.rt_hit;
            self.wrapup(m, z);
            let k = 256 * usize::from(op - LigKern::KERN) + usize::from(m.j.remainder);
            let w = m.font.kerns.get(k).copied().unwrap_or(0);
            self.emit.push(Item::Kern { w, sub: 0 });
            return L::Move;
        }
        if m.cur_l == NON_CHAR {
            m.lft_hit = true;
        } else if m.lig_stack.is_empty() {
            m.rt_hit = true;
        }
        let rem = i32::from(m.j.remainder);
        match op {
            1 | 5 => {
                m.cur_l = rem;
                m.i = m.font.glyph(m.cur_l).unwrap_or_default();
                m.ligature_present = true;
            }
            2 | 6 => {
                m.cur_r = rem;
                match m.lig_stack.last_mut() {
                    None => {
                        m.lig_stack.push(LigEntry::Item { ch: rem, ptr: None });
                        m.bchar = NON_CHAR;
                    }
                    Some(top @ LigEntry::Char(_)) => {
                        let LigEntry::Char(c) = *top else { unreachable!() };
                        *top = LigEntry::Item { ch: rem, ptr: Some(c) };
                    }
                    Some(LigEntry::Item { ch, .. }) => *ch = rem,
                }
            }
            3 => {
                m.cur_r = rem;
                m.lig_stack.push(LigEntry::Item { ch: rem, ptr: None });
            }
            7 | 11 => {
                self.wrapup(m, false);
                m.after_q = 0;
                m.cur_l = rem;
                m.i = m.font.glyph(m.cur_l).unwrap_or_default();
                m.ligature_present = true;
            }
            _ => {
                m.cur_l = rem;
                m.ligature_present = true;
                return if m.lig_stack.is_empty() { L::Wrapup } else { L::Move1 };
            }
        }
        if op > 4 && op != 7 {
            return L::Wrapup;
        }
        if m.cur_l < NON_CHAR {
            return L::LigLoop;
        }
        m.k = m.font.bchar_label.map_or(0, usize::from);
        L::LigLoop1
    }

    // ------------------------------------------------------------ spaces

    /// A space token in horizontal mode (tex.web 1030, 1041, 1043).
    pub(super) fn space_cmd(&mut self) {
        // (pdfTeX: with `\pdfadjustinterwordglue` positive, always the
        // normal space)
        if self.st.space_factor == 1000 || self.int_par(IntPar::Pdfadjustinterwordglue) > 0 {
            self.normal_space();
        } else {
            self.app_space();
        }
    }

    /// `\ ` (tex.web 1041): in vertical mode a paragraph begins first.
    pub(super) fn ex_space(&mut self) {
        if self.st.mode == Mode::V {
            let t = self.cmd.clone();
            self.back_new_graf(&t);
        } else {
            self.normal_space();
        }
    }

    /// §1042: the current font's interword glue.
    fn font_glue(&mut self) -> Glue {
        let f = self.font();
        let k = self.font_key(&f);
        Glue {
            width: self.int(&Key::FontDimen(k.clone(), 2)),
            stretch: self.int(&Key::FontDimen(k.clone(), 3)),
            shrink: self.int(&Key::FontDimen(k, 4)),
            ..Glue::default()
        }
    }

    /// §1041
    fn normal_space(&mut self) {
        let ss = self.glue_par(GluePar::Spaceskip);
        let it = if is_zero(&ss) {
            let g = self.font_glue();
            Item::Glue { g: Arc::new(g), sub: 0, zero: false }
        } else {
            param_glue(&ss, GluePar::Spaceskip as u8)
        };
        let it = if self.int_par(IntPar::Pdfadjustinterwordglue) > 0 { self.adjust_interword_glue(it) } else { it };
        self.emit.push(it);
    }

    /// pdftex.web's `adjust_interword_glue`: the glue after a character
    /// (the list's last item, or before the last if that is a
    /// `\pdfappendkern` kern) widened, stretched and shrunk by that
    /// character's `\knbscode`, `\stbscode`, `\shbscode` in thousandths
    /// of its font's quad.
    fn adjust_interword_glue(&mut self, it: Item) -> Item {
        let Item::Glue { g, sub, .. } = &it else { return it };
        if self.kind == Kind::Other {
            self.kind = Kind::Par;
        }
        let list = self.current_list(false);
        let ch = match list.last() {
            Some(Item::Kern { sub: 3, .. }) => list.len().checked_sub(2).and_then(|j| crate::micro::char_of(&list[j])),
            Some(x) => crate::micro::char_of(x),
            None => None,
        };
        let Some((f, c)) = ch.map(|(f, c)| (f.clone(), c)) else { return it };
        let k = self.font_key(&f);
        let mut codes = [0; 3];
        for (x, kind) in codes.iter_mut().zip([crate::micro::KNBS, crate::micro::STBS, crate::micro::SHBS]) {
            let t = self.font_code_table(&Key::FontCode(kind, k.clone()));
            *x = crate::micro::code(&t, kind, c);
        }
        if codes == [0; 3] {
            return it;
        }
        let quad = self.int(&Key::FontDimen(k, 6));
        let mut q = **g;
        q.width += Font::round_xn_over_d(quad, codes[0], 1000);
        q.stretch += Font::round_xn_over_d(quad, codes[1], 1000);
        q.shrink += Font::round_xn_over_d(quad, codes[2], 1000);
        Item::Glue { g: Arc::new(q), sub: *sub, zero: false }
    }

    /// §1043: `app_space`.
    fn app_space(&mut self) {
        let sf = self.st.space_factor;
        let xs = self.glue_par(GluePar::Xspaceskip);
        let it = if sf >= 2000 && !is_zero(&xs) {
            param_glue(&xs, GluePar::Xspaceskip as u8)
        } else {
            let ss = self.glue_par(GluePar::Spaceskip);
            let mut g = if is_zero(&ss) { self.font_glue() } else { *ss };
            // (§1044)
            if sf >= 2000 {
                let f = self.font();
                let k = self.font_key(&f);
                g.width += self.int(&Key::FontDimen(k, 7));
            }
            g.stretch = xn_over_d(g.stretch, sf, 1000).0;
            g.shrink = xn_over_d(g.shrink, 1000, sf).0;
            Item::Glue { g: Arc::new(g), sub: 0, zero: false }
        };
        self.emit.push(it);
    }

    // ------------------------------------------------------------ glue, rules, discretionaries

    /// tex.web 1060: `append_glue`.
    fn append_glue(&mut self, k: u8) {
        let (g, zero) = self.skip_glue(k);
        self.emit_here(Item::Glue { g: Arc::new(g), sub: 0, zero });
    }

    /// The glue of `\hskip` (0), `\hfil`, `\hfill`, `\hss`, `\hfilneg` (4)
    /// and their vertical twins, and whether it is a zero internal glue.
    pub(super) fn skip_glue(&mut self, k: u8) -> (Glue, bool) {
        let fil =
            |stretch: i32, shrink: i32| Glue { stretch, shrink, stretch_order: 1, shrink_order: 1, ..Glue::default() };
        match k {
            1 => (fil(65536, 0), false),
            2 => (Glue { stretch: 65536, stretch_order: 2, ..Glue::default() }, false),
            3 => (fil(65536, 65536), false),
            4 => (fil(-65536, 0), false),
            _ => {
                let g = self.scan_glue(false);
                (g, self.scratch.glue_shared && is_zero(&g))
            }
        }
    }

    /// tex.web 463: `scan_rule_spec`.
    pub(super) fn rule_spec(&mut self, hrule: bool) -> Item {
        let (mut w, mut h, mut d) = if hrule { (RUNNING, 26214, 0) } else { (26214, RUNNING, RUNNING) };
        loop {
            if self.keyword("width") {
                w = self.scan_dimen();
            } else if self.keyword("height") {
                h = self.scan_dimen();
            } else if self.keyword("depth") {
                d = self.scan_dimen();
            } else {
                break;
            }
        }
        Item::Rule { w, h, d }
    }

    /// A rule, and its effect on the list (tex.web 1056).
    fn rule(&mut self, hrule: bool) {
        let r = self.rule_spec(hrule);
        self.emit_here(r);
        if self.st.mode == Mode::V {
            self.vemit.push(Item::PrevDepth(IGNORE_DEPTH));
            self.st.prev_depth = IGNORE_DEPTH;
        } else {
            self.st.space_factor = 1000;
        }
    }

    /// tex.web 1113: `\/`.
    fn italic_correction(&mut self) {
        let w = match self.last_item() {
            Some(Item::Char(f, c)) => f.glyph(i32::from(c)).map(|g| g.italic),
            Some(Item::Lig(l)) => l.font.glyph(i32::from(l.c)).map(|g| g.italic),
            _ => None,
        };
        if let Some(w) = w {
            self.emit.push(Item::Kern { w, sub: 1 });
        }
    }

    /// tex.web 1117: `\-`, `\discretionary`.
    fn append_discretionary(&mut self, explicit: bool) {
        if !explicit {
            let f = self.font();
            let k = self.font_key(&f);
            let c = self.int(&Key::HyphenChar(k.clone()));
            let mut pre = Vec::new();
            if (0..256).contains(&c) {
                // (pdftex.web: the hyphen's `\pdfprependkern` and
                // `\pdfappendkern` kerns around it)
                let auto = self.auto_kern(&k);
                if let Some(x) = auto.as_ref().and_then(|a| a.kern(NON_CHAR, c)) {
                    pre.push(x);
                }
                if f.glyph(c).is_some() {
                    pre.push(Item::Char(f, c as u8));
                }
                if let Some(x) = auto.as_ref().and_then(|a| a.kern(c, NON_CHAR)) {
                    pre.push(x);
                }
            }
            self.emit.push(Item::Disc(Arc::new(Disc { pre, post: Vec::new(), replace: 0 })));
            return;
        }
        let g = BoxGroup {
            kind: BoxKind::Disc(0, Arc::new((Vec::new(), Vec::new()))),
            context: BoxContext::Shift(0),
            spec: Spec::NATURAL,
            adjusted: false,
            mode: self.st.mode,
            inner: self.st.inner,
            space_factor: self.st.space_factor,
            prev_depth: self.st.prev_depth,
            hyph_lang: self.st.hyph_lang,
            prev_graf: self.st.prev_graf,
        };
        self.scan_left_brace();
        self.open_box(g, Mode::H);
    }

    /// The next part of a discretionary begins (tex.web 1119).
    fn disc_next(&mut self) {
        let Some(be) = self.st.boxing.take() else { return };
        let mut g = (*be.g).clone();
        g.mode = self.st.mode;
        g.inner = self.st.inner;
        g.space_factor = self.st.space_factor;
        g.prev_depth = self.st.prev_depth;
        g.hyph_lang = self.st.hyph_lang;
        g.prev_graf = self.st.prev_graf;
        self.scan_left_brace();
        self.open_box(g, Mode::H);
    }

    // ------------------------------------------------------------ boxes

    /// Opens a box group: a scope, and a new list (tex.web 1083: `push_nest`).
    pub(super) fn open_box(&mut self, g: BoxGroup, mode: Mode) {
        let at = self.chain.nodes.len() as u32;
        self.st.groups.push(Group { kind: GroupKind::Box, after: Vec::new(), boxg: Some(Arc::new(g)), at, align: None });
        self.kind = Kind::Open;
        self.st.level += 1;
        self.st.mode = mode;
        self.st.inner = true;
        self.st.prev_graf = 0;
        if mode == Mode::V {
            self.st.prev_depth = IGNORE_DEPTH;
        } else {
            self.st.space_factor = 1000;
        }
    }

    /// `\setbox n=` (tex.web 1241).
    pub(super) fn setbox(&mut self, global: bool) {
        let n = self.scan_eight_bit_int();
        self.optional_equals();
        self.scan_box(BoxContext::Set(n, global));
    }

    /// tex.web 1084: `scan_box`.
    fn scan_box(&mut self, context: BoxContext) {
        match self.nonblank_nonrelax() {
            Some((_, Meaning::Prim(Prim::Mat(Mat::MakeBox(k))))) => self.begin_box(k, context),
            // (leaders of a rule, tex.web 1084)
            Some((_, Meaning::Prim(Prim::Mat(r @ (Mat::Hrule | Mat::Vrule)))))
                if matches!(context, BoxContext::Leaders(_)) =>
            {
                let rule = self.rule_spec(r == Mat::Hrule);
                if let BoxContext::Leaders(sub) = context {
                    self.leaders_after(sub, rule);
                }
            }
            other => {
                self.err("A <box> was supposed to be here".into());
                if let Some((t, _)) = other {
                    self.back(t);
                }
            }
        }
    }

    /// tex.web 1079: `begin_box`.
    fn begin_box(&mut self, k: u8, context: BoxContext) {
        match k {
            0 | 1 => {
                let n = self.scan_eight_bit_int();
                let b = match self.get(&Key::Box(n)) {
                    Val::Box(b) => Some(b),
                    _ => None,
                };
                if k == 0 && b.is_some() {
                    self.change_box(n, Val::Void);
                }
                self.box_end(context, b.map(|b| (*b).clone()), Vec::new(), Vec::new());
            }
            2 => {
                let b = self.last_box();
                self.box_end(context, b, Vec::new(), Vec::new());
            }
            3 => {
                let b = self.vsplit_box();
                self.box_end(context, b, Vec::new(), Vec::new());
            }
            _ => {
                let kind = match k {
                    4 => BoxKind::Vtop,
                    5 => BoxKind::V,
                    _ => BoxKind::H,
                };
                let adjusted = kind == BoxKind::H && matches!(context, BoxContext::Shift(_)) && self.st.mode == Mode::V;
                // (§645: `scan_spec`)
                let spec = if self.keyword("to") {
                    Spec::Exactly(self.scan_dimen())
                } else if self.keyword("spread") {
                    Spec::Additional(self.scan_dimen())
                } else {
                    Spec::NATURAL
                };
                self.scan_left_brace();
                let vertical = kind != BoxKind::H;
                // (`normal_paragraph` inside the group, if it defines anything)
                let shaped = vertical && self.paragraph_shaped();
                let g = BoxGroup {
                    kind,
                    context,
                    spec,
                    adjusted,
                    mode: self.st.mode,
                    inner: self.st.inner,
                    space_factor: self.st.space_factor,
                    prev_depth: self.st.prev_depth,
            hyph_lang: self.st.hyph_lang,
            prev_graf: self.st.prev_graf,
                };
                self.open_box(g, if vertical { Mode::V } else { Mode::H });
                let every = self.toks_par(if vertical { TokPar::Everyvbox } else { TokPar::Everyhbox });
                self.rewrite(every);
                if shaped {
                    let t = self.cs("\u{0}normalparagraph");
                    self.back(t);
                }
            }
        }
    }

    /// A box register's new value where TeX assigns it without the save
    /// stack (`box(n):=null`, tex.web 1079, 1110, 1247): a definition at
    /// the level of the one it replaces (CORE.md Conflict 7): global when
    /// that one was global, made at the piece's outer level or before the
    /// piece (pieces are cut at depth 0: no group spans them); else living
    /// as long as its group.
    pub(super) fn change_box(&mut self, n: u16, val: Val) {
        self.change_direct(Key::Box(n), val);
    }

    /// A value tex.web assigns directly (a box register's, the
    /// page builder's quantities, tex.web 1245–1246): a definition at the
    /// level of the one it replaces ([`Exec::change_box`]).
    pub(super) fn change_direct(&mut self, key: Key, val: Val) {
        // (this node's own earlier definition, if any)
        if let Some(i) = self.defs.iter().rposition(|d| d.key == key) {
            let (global, level) = (self.defs[i].global, self.defs[i].level);
            self.defs.push(super::Def { key, val, global, level });
            return;
        }
        // (the definition it replaces, read: a kept node is checked by its
        // value and its scope)
        let _ = self.lookup(&key);
        let scope = match self.chain.resolve(&key, self.chain.nodes.len()) {
            Some((d, _)) => {
                let node = &self.chain.nodes[d as usize];
                match node.defs().iter().rev().find(|x| x.key == key) {
                    Some(def) if !def.global => self.chain.def_group(node, def),
                    _ => None,
                }
            }
            None => None,
        };
        let open = self.chain.open_groups();
        let level = scope.and_then(|g| open.iter().rposition(|&o| o == g).map(|p| (open.len() - 1 - p) as u16));
        match (scope, level) {
            (Some(_), Some(0)) => self.define(key, val, false),
            (Some(_), Some(k)) => self.defs.push(super::Def { key, val, global: false, level: Some(k) }),
            _ => self.define(key, val, !self.st.groups.is_empty()),
        }
    }

    /// `\wd n=d` and friends (tex.web 1247).
    pub(super) fn alter_box_dimen(&mut self, c: u8) {
        let n = self.scan_eight_bit_int();
        self.optional_equals();
        let d = self.scan_dimen();
        if let Val::Box(b) = self.get(&Key::Box(n)) {
            let mut b = (*b).clone();
            match c {
                0 => b.w = d,
                1 => b.h = d,
                _ => b.d = d,
            }
            self.change_box(n, Val::Box(Arc::new(b)));
        }
    }

    /// `\wd n`: a box's dimension (tex.web 420).
    pub(super) fn box_dimen(&mut self, c: u8) -> i32 {
        let n = self.scan_eight_bit_int();
        match self.get(&Key::Box(n)) {
            Val::Box(b) => match c {
                0 => b.w,
                1 => b.h,
                _ => b.d,
            },
            _ => 0,
        }
    }

    /// `}` of a box group (tex.web 1085): a `Close` node; the packaging
    /// node after it makes the box. In a `\vbox` a paragraph ends first.
    pub(super) fn close_box(&mut self, t: &Tok) {
        let Some(g) = self.st.groups.last().and_then(|g| g.boxg.clone()) else { return };
        if self.st.mode == Mode::H && !self.st.inner {
            // (the `}` read again: as `back_input`, which an alignment's
            // brace count sees)
            let e = self.cs("\u{0}endgraf");
            self.back(t.clone());
            self.back(e);
            return;
        }
        let boxmaxdepth = self.dimen_par(DimenPar::Boxmaxdepth);
        // (an insertion's parameters as they are inside it, tex.web 1100)
        let g = match &g.kind {
            BoxKind::Insert(n, _) => {
                let p = InsParams {
                    split_top: self.glue_par(GluePar::Splittopskip),
                    depth: self.dimen_par(DimenPar::Splitmaxdepth),
                    float: self.int_par(IntPar::Floatingpenalty),
                };
                Arc::new(BoxGroup { kind: BoxKind::Insert(*n, Some(Arc::new(p))), ..(*g).clone() })
            }
            _ => g,
        };
        let after = self.st.groups.pop().map(|g| g.after).unwrap_or_default();
        self.st.mode = g.mode;
        self.st.inner = g.inner;
        self.st.space_factor = g.space_factor;
        self.st.prev_depth = g.prev_depth;
        self.st.hyph_lang = g.hyph_lang;
        self.st.prev_graf = g.prev_graf;
        self.st.level -= 1;
        self.st.boxing = Some(Arc::new(BoxEnd { g, boxmaxdepth, math: None }));
        self.rewrite(after);
        let p = self.cs("\u{0}package");
        self.back(p);
        self.closes();
    }

    /// The box a group closed just before, from the items its nodes
    /// emitted (tex.web 1086: `package`; 1119: `build_discretionary`).
    fn package(&mut self) {
        let Some(be) = self.st.boxing.take() else { return };
        self.kind = Kind::Par;
        let inner = self.st.level + 1;
        let vertical = matches!(be.g.kind, BoxKind::V | BoxKind::Vtop | BoxKind::Insert(..) | BoxKind::Vcenter);
        let nodes = &self.chain.nodes;
        // (the nodes inside: back from the `Close` node before this one)
        let end = nodes.len().saturating_sub(1);
        let start = box_start(self.chain, end, inner);
        let me = nodes.len();
        let mut list = Vec::new();
        // (which node made each item: its distance back from this one)
        let mut orig: Vec<u32> = Vec::new();
        let mut pd = IGNORE_DEPTH;
        for (j, n) in nodes.range(start, end).enumerate() {
            if n.out.level == inner {
                let d = (me - start - j) as u32;
                if vertical {
                    for it in n.vemit() {
                        push_vertical(&mut list, &mut pd, it.clone());
                        orig.resize(list.len(), d);
                    }
                } else {
                    for it in n.emit.iter() {
                        push_horizontal(&mut list, it.clone());
                        orig.resize(list.len(), d);
                    }
                }
            }
        }
        let ships: Vec<Item> = list.iter().filter(|i| matches!(i, Item::Shipout(_))).cloned().collect();
        let mut keep = list.iter().map(|i| !matches!(i, Item::Shipout(_) | Item::BuildPage(_) | Item::End(_)));
        orig.retain(|_| keep.next().unwrap_or(true));
        list.retain(|i| !matches!(i, Item::Shipout(_) | Item::BuildPage(_) | Item::End(_)));
        // (the nodes, once the packager moved the marks and insertions out
        // or not: [`Origins`])
        let origins_of = |orig: &[u32], list: &[Item], adjusted: bool| {
            Origins::from_runs(
                list.iter()
                    .zip(orig)
                    .filter(|(i, _)| !adjusted || !matches!(i, Item::Mark(_) | Item::Marks(..) | Item::Ins(_) | Item::Adjust(_)))
                    .enumerate()
                    .map(|(k, (_, &d))| (k as u32, d)),
            )
        };
        match &be.g.kind {
            BoxKind::Vcenter => self.vcenter_package(&be, list, ships),
            BoxKind::Math(k) => {
                let k = k.clone();
                self.math_package(&be, &k, list, ships);
            }
            BoxKind::H => {
                let params = Params {
                    badness: self.int_par(IntPar::Hbadness),
                    fuzz: self.dimen_par(DimenPar::Hfuzz),
                    overfull_rule: self.dimen_par(DimenPar::Overfullrule),
                };
                let mut adj = Vec::new();
                let mut list = list;
                if self.int_par(IntPar::TeXXeTstate) > 0 {
                    // (etex.web: TeXXeT's checks of `hpack`)
                    crate::pack::lr_fix(&mut list);
                }
                let origins = if orig.len() == list.len() { origins_of(&orig, &list, be.g.adjusted) } else { Origins::default() };
                let mut packed = hpack(list, be.g.spec, &params, if be.g.adjusted { Some(&mut adj) } else { None });
                packed.b.origins = origins;
                if let Some(r) = packed.report {
                    let cx = self.show_ctx(std::slice::from_ref(&Item::Box(Arc::new(packed.b.clone()))));
                    let s = report(r, &packed.b, self.box_place(), &cx);
                    self.log_raw(&s);
                }
                self.box_end(be.g.context, Some(packed.b), adj, ships);
            }
            BoxKind::V | BoxKind::Vtop => {
                let params = Params {
                    badness: self.int_par(IntPar::Vbadness),
                    fuzz: self.dimen_par(DimenPar::Vfuzz),
                    overfull_rule: 0,
                };
                let origins = origins_of(&orig, &list, false);
                let mut packed = vpack(list, be.g.spec, be.boxmaxdepth, &params);
                packed.b.origins = origins;
                if let Some(r) = packed.report {
                    let cx = self.show_ctx(std::slice::from_ref(&Item::Box(Arc::new(packed.b.clone()))));
                    let s = report(r, &packed.b, self.box_place(), &cx);
                    self.log_raw(&s);
                }
                let mut b = packed.b;
                if be.g.kind == BoxKind::Vtop {
                    // (§1087)
                    let h = match b.list.first() {
                        Some(Item::Box(x)) => x.h,
                        Some(Item::Rule { h, .. }) => *h,
                        _ => 0,
                    };
                    b.d = b.d - h + b.h;
                    b.h = h;
                }
                self.box_end(be.g.context, Some(b), Vec::new(), ships);
            }
            BoxKind::Insert(n, p) => self.insert_end(*n, p.as_deref(), list, ships),
            BoxKind::Disc(stage, parts) => {
                for s in ships {
                    self.emit_here(s);
                }
                // (§1121: only characters, kerns, boxes, rules, ligatures)
                if let Some(bad) = list.iter().position(|i| {
                    !matches!(i, Item::Char(..) | Item::Kern { .. } | Item::Box(_) | Item::Rule { .. } | Item::Lig(_))
                }) {
                    self.err("Improper discretionary list".into());
                    list.truncate(bad);
                }
                match stage {
                    0 | 1 => {
                        let parts = if *stage == 0 { (list, Vec::new()) } else { (parts.0.clone(), list) };
                        let mut g = (*be.g).clone();
                        g.kind = BoxKind::Disc(stage + 1, Arc::new(parts));
                        self.st.boxing =
                            Some(Arc::new(BoxEnd { g: Arc::new(g), boxmaxdepth: be.boxmaxdepth, math: None }));
                        let t = self.cs("\u{0}discnext");
                        self.back(t);
                    }
                    _ => {
                        // (§1120: in a formula the third part must be empty)
                        if !list.is_empty() && self.st.mode == Mode::M {
                            self.error(
                                "Illegal math \\discretionary",
                                &[
                                    "Sorry: The third part of a discretionary break must be",
                                    "empty, in math formulas. I had to delete your third part.",
                                ],
                            );
                            list.clear();
                        }
                        let n = list.len();
                        if n > 255 {
                            self.err("Discretionary list is too long".into());
                            list.clear();
                        }
                        let d = Disc { pre: parts.0.clone(), post: parts.1.clone(), replace: list.len() as u8 };
                        self.emit.push(Item::Disc(Arc::new(d)));
                        self.emit.extend(list);
                    }
                }
            }
        }
    }

    /// tex.web 1075: `box_end`.
    pub(super) fn box_end(&mut self, context: BoxContext, b: Option<BoxNode>, adj: Vec<Item>, ships: Vec<Item>) {
        for s in ships {
            self.emit_here(s);
        }
        let Some(mut b) = b else {
            if let BoxContext::Set(n, global) = context {
                self.define(Key::Box(n), Val::Void, global);
            }
            return;
        };
        match context {
            BoxContext::Shift(s) => {
                b.shift = s;
                let b = Arc::new(b);
                if self.st.mode == Mode::V {
                    let inter = self.interline();
                    self.st.prev_depth = b.d;
                    self.vemit.push(Item::Append(b, inter));
                    self.vemit.extend(adj);
                    self.build_page();
                } else if self.st.mode == Mode::M {
                    // (§1076: in math, an ord noad around it)
                    self.emit.push(super::math::box_noad(b));
                } else {
                    self.st.space_factor = 1000;
                    self.emit.push(Item::Box(b));
                }
            }
            BoxContext::Set(n, global) => self.define(Key::Box(n), Val::Box(Arc::new(b)), global),
            BoxContext::Ship => {
                // (tex.web 638: `dead_cycles:=0`)
                self.define(Key::Page(super::output::DEAD_CYCLES), Val::Int(0), true);
                // (§617: the first page fixes the magnification; here, the
                // first while none is fixed)
                if self.int(&Key::MagSet) == 0 {
                    self.prepare_mag();
                }
                let ship = self.ship_params();
                // (§641: a page too large is not shipped, nor its whatsits
                // done)
                let max = i64::from(super::scan::MAX_DIMEN);
                let (h, d, w) = (i64::from(b.h), i64::from(b.d), i64::from(b.w));
                if h > max || d > max || h + d + i64::from(ship.voffset) > max || w + i64::from(ship.hoffset) > max {
                    let mark = page_mark(&ship);
                    self.log_raw(&mark);
                    self.err("Huge page cannot be shipped out".into());
                    return;
                }
                // (the page's whatsits are done as it is shipped, here:
                // its `\write`s expanded with the definitions reaching this
                // node, tex.web 1374)
                let io = self.ship_io(&b);
                let mut mark = page_mark(&ship);
                if self.int_par(IntPar::Tracingoutput) > 0 {
                    // (§638–§639: the box displayed before it is shipped)
                    use crate::show::{LN, NL, SEL_POP, SEL_PUSH};
                    let cur = self.selector();
                    let diag = if cur == crate::print::TERM_AND_LOG && self.int_par(IntPar::Tracingonline) <= 0 {
                        crate::print::LOG_ONLY
                    } else {
                        cur
                    };
                    let mut s = String::from(RAW);
                    s.push(crate::print::sel(cur));
                    s.push(NL);
                    s.push(LN);
                    s.push_str("Completed box being shipped out");
                    s.push_str(mark.strip_prefix(RAW).unwrap_or(&mark));
                    s.push(SEL_PUSH);
                    s.push(crate::print::sel(diag));
                    let bx = Arc::new(b.clone());
                    let cx = self.show_ctx(std::slice::from_ref(&Item::Box(bx.clone())));
                    s.push(NL);
                    crate::show::show_box(&mut s, &bx, &cx);
                    s.push(NL);
                    s.push(LN);
                    s.push(SEL_POP);
                    s.push_str(&io);
                    mark = s;
                } else {
                    mark.pop();
                    mark.push_str(&io);
                    mark.push(']');
                }
                self.log_raw(&mark);
                self.emit_here(Item::Shipout(Arc::new(Shipped { b: Arc::new(b), ship })));
            }
            BoxContext::Leaders(sub) => self.leaders_after(sub, Item::Box(Arc::new(b))),
        }
    }

    /// `\unhbox`, `\unvbox` and their copies (tex.web 1110).
    fn unpackage(&mut self, copy: bool, vertical: bool) {
        let n = self.scan_eight_bit_int();
        let Val::Box(b) = self.get(&Key::Box(n)) else { return };
        if b.vertical != vertical {
            self.err("Incompatible list can't be unboxed".into());
            return;
        }
        if !copy {
            self.change_box(n, Val::Void);
        }
        // (pdftex.web 1110: the margin kerns are dropped)
        for it in b.list.iter().filter(|it| !matches!(it, Item::MarginKern { .. })) {
            self.emit_here(it.clone());
        }
    }

    // ------------------------------------------------------------ the end

    /// `\end` (tex.web 1054): on the main vertical list, the residual
    /// material goes out and the reading stops.
    pub(super) fn stop(&mut self) {
        let t = self.cmd.clone();
        if self.st.mode == Mode::H {
            self.head_for_vmode(&t, false);
            return;
        }
        if self.st.level > 0 {
            self.err(format!("You can't use `{}' in internal vertical mode", self.shown(&t)));
            return;
        }
        let c = self.page_call();
        self.vemit.push(Item::End(c));
        // (the end of the job is a definition: the pieces after it read it
        // at their first node and read nothing more)
        self.define(Key::Ended, Val::Int(1), true);
        self.st.pending.clear();
        self.st.texts = super::Stack::new();
        self.st.pos = self.text.len();
    }
}

/// The main vertical list's last item after paragraph `p`'s contributions
/// (its items before its lines, its lines, its items after them): the
/// value of `Key::MainTail` (`Val::Void`: none, a job begun), if they
/// change it.
pub(super) fn main_tail(p: &Para) -> Option<Val> {
    let mut list = Vec::new();
    let mut pd = IGNORE_DEPTH;
    let mut changed = false;
    let mut add = |list: &mut Vec<Item>, it: &Item| match it {
        Item::BuildPage(_) | Item::End(_) | Item::Shipout(_) | Item::Halt => {}
        Item::JobStart => {
            list.clear();
            changed = true;
        }
        _ => {
            push_vertical(list, &mut pd, it.clone());
            changed = true;
        }
    };
    for it in &p.before {
        add(&mut list, it);
    }
    if p.lb.is_some() {
        // (a line of the paragraph: a box, whatever line breaking makes)
        add(&mut list, &Item::Box(Arc::new(BoxNode::default())));
    }
    for it in &p.after {
        add(&mut list, it);
    }
    if !changed {
        return None;
    }
    Some(list.pop().map_or(Val::Void, |t| Val::Tail(Arc::new(t))))
}

/// A `\shipout` node, as PDF shipping reads where it is.
struct ExecShip<'x, 'a>(&'x mut Exec<'a>);

impl crate::pdf::ShipEnv for ExecShip<'_, '_> {
    fn read(&mut self, key: &Key) -> crate::val::Val {
        self.0.get(key)
    }
    fn text(&mut self, toks: &[crate::val::Tok]) -> Vec<u8> {
        let mut s = String::new();
        self.0.show_toks(&mut s, toks);
        s.chars().map(|c| u32::from(c).min(255) as u8).collect()
    }
}
