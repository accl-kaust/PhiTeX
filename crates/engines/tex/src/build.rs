//! Building lists and boxes (§1029–§1135): the mode as a binding, the
//! current list as the contributions tagged with its list, characters
//! and ligatures (§1034–§1040), spaces, paragraphs and their line
//! breaking, boxes, insertions, discretionaries and accents.
//!
//! There is no mode variable and no current list: the mode is read from
//! the scope kind and the field `par:<list>`; a list is the contributions
//! to the accumulator `items` that carry its id.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use phitex_defuse::{Brackets, Effect, Value, ValueId};
use phitex_typeset::glue::Glue;
use phitex_typeset::node::{BoxKind, BoxNode, Disc, GlueKind, KernKind, Ligature, List, Node};
use phitex_typeset::pack::{PackParams, Spec};

use crate::control::{LEADER_FLAG, SHIP_OUT_FLAG};
use crate::nodes::{self, Side, T_BUILD_PAGE, T_DELETE};
use crate::run::{R, Run};
use crate::val::{Tok, cmd, glue_of, group, int_of, list_of};

pub(crate) const IGNORE_DEPTH: i32 = -65_536_000;
const BOX_FLAG: i64 = 0o10000000000;

/// What a command reads of its surroundings: the mode (tex.web's
/// numbers: ±1 vertical, ±102 horizontal, ±203 math), the list it builds
/// (`lid`), the vertical list a paragraph belongs to (`vlid`), and the
/// innermost group.
#[derive(Clone, Debug)]
pub(crate) struct Mode {
    pub m: i32,
    /// The mode of the scope kind (a paragraph is not a scope).
    pub km: i32,
    pub lid: i64,
    pub vlid: i64,
    pub group: i64,
    pub sid: i64,
    pub depth: i64,
    pub extra: Vec<ValueId>,
    /// The paragraph's record `[list, clang]`, in a paragraph.
    pub para: Option<(i64, i64)>,
    pub vm: i32,
}

/// A box under construction (`scan_spec` §645 and the context, §1083).
pub(crate) fn null_box(v: &phitex_defuse::Values, w: i32) -> ValueId {
    let b = BoxNode {
        width: w,
        ..BoxNode::default()
    };
    nodes::encode_box(v, &b, &Side::default())
}

impl Run<'_, '_> {
    /// The mode and current list: the scope kind, and in vertical mode
    /// whether a paragraph is open (§211).
    pub(crate) fn mode(&mut self) -> R<Mode> {
        let k = self.scope_kind()?;
        let v = self.v;
        let (group, sid, lid, km, depth, extra) = match k {
            Some(k) => {
                let xs = list_of(v, k);
                let n = |i: usize| xs.get(i).map_or(0, |&x| int_of(v, x));
                (
                    n(0),
                    n(1),
                    n(2),
                    i32::try_from(n(3)).unwrap_or(1),
                    n(4),
                    xs.get(6..).map(<[ValueId]>::to_vec).unwrap_or_default(),
                )
            }
            None => (0, 0, 0, 1, 0, Vec::new()),
        };
        let mut md = Mode {
            m: km,
            km,
            lid,
            vlid: lid,
            group,
            sid,
            depth,
            extra,
            para: None,
            vm: km,
        };
        if km.abs() == 1
            && let Some(p) = self.get(&format!("par:{lid}"))?
            && p != ValueId::UNIT
        {
            let xs = list_of(v, p);
            let id = xs.first().map_or(0, |&x| int_of(v, x));
            let lang = xs.get(1).map_or(0, |&x| int_of(v, x));
            md.m = 102;
            md.lid = id;
            md.para = Some((id, lang));
        }
        Ok(md)
    }

    /// A scope kind: `[group, sid, list, mode, depth, parent kind,
    /// extra…]`; the parent is the kind the group's end returns to.
    pub(crate) fn kind_value(&mut self, md: &Mode, grp: i64, sid: i64, lid: i64, m: i32, extra: Vec<ValueId>) -> R<ValueId> {
        let parent = self.scope_kind()?.unwrap_or(ValueId::UNIT);
        Ok(self.kind_with_parent(md, grp, sid, lid, m, extra, parent))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn kind_with_parent(
        &self,
        md: &Mode,
        grp: i64,
        sid: i64,
        lid: i64,
        m: i32,
        extra: Vec<ValueId>,
        parent: ValueId,
    ) -> ValueId {
        let v = self.v;
        let mut xs = vec![
            v.int(grp),
            v.int(sid),
            v.int(lid),
            v.int(i64::from(m)),
            v.int(md.depth + 1),
            parent,
        ];
        xs.extend(extra);
        v.list(xs)
    }

    /// Opens a group whose scope includes this node (`new_save_level`,
    /// §274).
    pub(crate) fn open_scope_here(&mut self, k: ValueId) {
        self.effect = Effect::Scope(Brackets {
            closes: 0,
            opens: 1,
            before: true,
            kind: Some(k),
        });
    }

    /// `{` and `\begingroup` (§1063).
    pub(crate) fn open_group(&mut self, md: &Mode, grp: i64, extra: Vec<ValueId>) -> R<()> {
        let sid = self.node_raw();
        let k = self.kind_value(md, grp, sid, md.vlid, md.km, extra)?;
        self.open_scope_here(k);
        Ok(())
    }

    /// The end of a simple group: `unsave` (§281–§282) is the engine's;
    /// the `\aftergroup` tokens and the definitions that must reach out
    /// of the group are this node's.
    pub(crate) fn close_simple(&mut self, md: &Mode) -> R<()> {
        self.effect = Effect::Close;
        self.unsave_extras(md)
    }

    /// What `unsave` does besides restoring: the `\aftergroup` tokens
    /// (§282), and box registers changed "at the same level" (§1079,
    /// §1247), defined again outside.
    pub(crate) fn unsave_extras(&mut self, md: &Mode) -> R<()> {
        let ag = self.scope_list("ag", md)?;
        let toks: Vec<Tok> = ag.iter().map(|&t| Tok::decode(self.v, t)).collect();
        self.push_toks(toks);
        let fix = self.scope_list("lvlfix", md)?;
        if !fix.is_empty() {
            let v = self.v;
            let mut defs = Vec::new();
            let mut carry = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for f in fix {
                let name = crate::val::str_of(v, f);
                if !seen.insert(name.clone()) {
                    continue;
                }
                let val = self.get(&name)?.unwrap_or(ValueId::UNIT);
                let lvl = crate::val::int_at(v, val, 0);
                defs.push(v.list(vec![v.str(&name), val, v.int(0)]));
                if lvl < md.depth {
                    carry.push(v.str(&name));
                }
            }
            let put = self.ops.put;
            self.kid(put, vec![v.list(defs), v.list(carry)]);
        }
        Ok(())
    }

    /// A list field belonging to the innermost group (`[sid, items…]`).
    pub(crate) fn scope_list(&mut self, name: &str, md: &Mode) -> R<Vec<ValueId>> {
        let x = self.get(name)?;
        let v = self.v;
        Ok(match x {
            Some(x) => {
                let xs = list_of(v, x);
                if xs.first().is_some_and(|&s| int_of(v, s) == md.sid) {
                    xs[1..].to_vec()
                } else {
                    Vec::new()
                }
            }
            None => Vec::new(),
        })
    }

    pub(crate) fn set_scope_list(&mut self, name: &str, md: &Mode, items: Vec<ValueId>) {
        let v = self.v;
        let mut xs = vec![v.int(md.sid)];
        xs.extend(items);
        self.set(name, v.list(xs), false);
    }

    // ------------------------------------------------------------ lists

    /// `tail_append` to list `lid`.
    pub(crate) fn append(&mut self, lid: i64, node: ValueId) {
        let v = self.v;
        let e = v.list(vec![v.int(lid), node]);
        let s = v.seq_unit(e);
        self.contribute("items", s);
    }

    /// A point where tex.web calls `build_page` on the main list.
    pub(crate) fn build_page_mark(&mut self, md: &Mode) -> R<()> {
        self.build_page(md)
    }

    /// The contents of list `lid` so far, and how many of them are
    /// still contributions (after the last `build_page`).
    // REDUCE-GAP: acc_range anchors only at a node id the run holds;
    // a list's opener is known as a value, so its readers fold from the
    // unit's start and keep their list's items.
    pub(crate) fn list_items(&mut self, lid: i64) -> R<(Vec<ValueId>, usize)> {
        let v = self.v;
        let f = v.field("items");
        let all = self.cx.acc_range(f, None)?;
        let mut out: Vec<ValueId> = Vec::new();
        let mut since_bp = 0usize;
        let own: Vec<ValueId> = self
            .contribs
            .iter()
            .filter(|(g, _)| *g == f)
            .map(|(_, x)| *x)
            .collect();
        let mut push = |x: ValueId, out: &mut Vec<ValueId>| {
            let Value::List(xs) = &*v.get(x) else { return };
            if xs.len() != 2 || int_of(v, xs[0]) != lid {
                return;
            }
            let n = xs[1];
            match nodes::tag(v, n) {
                T_DELETE => {
                    if since_bp > 0 {
                        out.pop();
                        since_bp -= 1;
                    }
                }
                T_BUILD_PAGE => since_bp = 0,
                _ => {
                    out.push(n);
                    since_bp += 1;
                }
            }
        };
        for x in v.seq_to_vec(all) {
            push(x, &mut out);
        }
        for s in own {
            for x in v.seq_to_vec(s) {
                push(x, &mut out);
            }
        }
        Ok((out, since_bp))
    }

    pub(crate) fn prev_depth(&mut self, md: &Mode) -> R<i32> {
        let x = self.get(&format!("pd:{}", md.vlid))?;
        Ok(x.map_or(IGNORE_DEPTH, |x| i32::try_from(int_of(self.v, x)).unwrap_or(0)))
    }

    pub(crate) fn set_prev_depth(&mut self, md: &Mode, d: i32) {
        let v = self.v.int(i64::from(d));
        self.set(&format!("pd:{}", md.vlid), v, true);
    }

    pub(crate) fn space_factor(&mut self, md: &Mode) -> R<i32> {
        let x = self.get(&format!("spf:{}", md.lid))?;
        Ok(x.map_or(1000, |x| i32::try_from(int_of(self.v, x)).unwrap_or(1000)))
    }

    pub(crate) fn set_space_factor(&mut self, md: &Mode, s: i32) {
        let v = self.v.int(i64::from(s));
        self.set(&format!("spf:{}", md.lid), v, true);
    }

    /// `append_to_vlist` (§679).
    pub(crate) fn append_to_vlist(&mut self, md: &Mode, b: ValueId) -> R<()> {
        let pd = self.prev_depth(md)?;
        let ht = nodes::box_dim(self.v, b, 3);
        let bs = self.glue_param("baselineskip")?;
        let ls = self.glue_param("lineskip")?;
        let lsl = self.param("lineskiplimit")?;
        if let Some(g) = phitex_typeset::linebreak::interline_glue(pd, ht, bs, ls, lsl) {
            let gv = nodes::encode(self.v, &g, &Side::default());
            self.append(md.vlid, gv);
        }
        self.append(md.vlid, b);
        let d = nodes::box_dim(self.v, b, 2);
        self.set_prev_depth(md, d);
        Ok(())
    }

    // -------------------------------------------------------- paragraphs

    /// `norm_min` (§1091).
    fn norm_min(h: i32) -> i64 {
        i64::from(h.clamp(1, 63))
    }

    /// `new_graf` (§1091).
    pub(crate) fn new_graf(&mut self, md: &Mode, indented: bool) -> R<()> {
        let v = self.v;
        self.set(&format!("pg:{}", md.vlid), v.int(0), true);
        let nonempty = if md.km == 1 { true } else { !self.list_items(md.vlid)?.0.is_empty() };
        if nonempty {
            let g = self.glue_param("parskip")?;
            let n = nodes::glue_node(v, &g, GlueKind::Param(2));
            self.append(md.vlid, n);
        }
        let id = self.node_raw();
        let lang = i64::from(phitex_typeset::hyph::cur_lang(self.param("language")?));
        let lhm = Self::norm_min(self.param("lefthyphenmin")?);
        let rhm = Self::norm_min(self.param("righthyphenmin")?);
        let packed = (lhm * 64 + rhm) * 65536 + lang;
        let rec = v.list(vec![v.int(id), v.int(lang), v.int(packed)]);
        self.set(&format!("par:{}", md.vlid), rec, true);
        if indented {
            let w = self.param("parindent")?;
            let b = null_box(v, w);
            self.append(id, b);
        }
        let ep = self.get("param:everypar")?;
        let toks = crate::val::toks_of(v, ep);
        self.push_toks(toks);
        if md.km == 1 && md.depth == 0 || md.lid == 0 && md.km == 1 {
            self.build_page_mark(md)?;
        }
        Ok(())
    }

    /// `normal_paragraph` (§1070).
    pub(crate) fn normal_paragraph(&mut self) -> R<()> {
        let v = self.v;
        if self.param("looseness")? != 0 {
            self.set("param:looseness", v.int(0), false);
        }
        if self.param("hangindent")? != 0 {
            self.set("param:hangindent", v.int(0), false);
        }
        if self.param("hangafter")? != 1 {
            self.set("param:hangafter", v.int(1), false);
        }
        if self.get("parshape")?.is_some_and(|x| !list_of(v, x).is_empty()) {
            self.set("parshape", v.list(Vec::new()), false);
        }
        Ok(())
    }

    /// `head_for_vmode` (§1095).
    pub(crate) fn head_for_vmode(&mut self, md: &Mode) -> R<()> {
        if md.m < 0 {
            // off_save (or "You can't use \hrule here")
            return Ok(());
        }
        self.back_cur();
        self.back_input(Tok::cs("par"));
        Ok(())
    }

    /// `end_graf` (§1096).
    pub(crate) fn end_graf(&mut self, md: &Mode) -> R<()> {
        if md.m == 102 {
            let (items, _) = self.list_items(md.lid)?;
            let v = self.v;
            if !items.is_empty() {
                self.line_break(md, &items)?;
            }
            self.set(&format!("par:{}", md.vlid), ValueId::UNIT, true);
            self.normal_paragraph()?;
            let _ = v;
        }
        Ok(())
    }

    /// The fonts the nodes use.
    pub(crate) fn font_view(&mut self, list: &[Node]) -> R<crate::fonts::View> {
        let mut ids = std::collections::BTreeSet::new();
        fn walk(l: &[Node], ids: &mut std::collections::BTreeSet<u32>) {
            for n in l {
                match n {
                    Node::Char { font, .. } => {
                        ids.insert(*font);
                    }
                    Node::Lig(g) => {
                        ids.insert(g.font);
                    }
                    Node::Box(b) => walk(&b.list, ids),
                    Node::Disc(d) => {
                        walk(&d.pre, ids);
                        walk(&d.post, ids);
                        walk(&d.replace, ids);
                    }
                    Node::Ins(i) => walk(&i.list, ids),
                    Node::Adjust(a) => walk(a, ids),
                    Node::Glue(g) => {
                        if let Some(l) = &g.leader {
                            walk(std::slice::from_ref(l), ids);
                        }
                    }
                    _ => {}
                }
            }
        }
        walk(list, &mut ids);
        let mut view = crate::fonts::View::default();
        for f in ids {
            self.add_font(&mut view, f)?;
        }
        Ok(view)
    }

    pub(crate) fn add_font(&mut self, view: &mut crate::fonts::View, f: u32) -> R<()> {
        if view.fonts.contains_key(&f) || f == 0 {
            return Ok(());
        }
        let info = self.get(&format!("fontinfo:{f}"))?;
        let Some(i) = crate::fonts::info(self.v, info) else {
            return Ok(());
        };
        let Some(file) = i.file else { return Ok(()) };
        let Some(l) = crate::fonts::load(self.v, file, i.s) else {
            return Ok(());
        };
        let n = self.int32(&format!("fdn:{f}"))?;
        let mut params = Vec::new();
        for k in 1..=n {
            params.push(self.int32(&format!("fd:{f}:{k}"))?);
        }
        let hc = self.int32(&format!("hc:{f}"))?;
        view.hyphen.insert(f, hc);
        view.names.insert(f, (i.name.clone(), i.size));
        view.fonts.insert(f, (l, params));
        Ok(())
    }

    pub(crate) fn pack_params(&mut self, h: bool) -> R<PackParams> {
        Ok(if h {
            PackParams {
                badness: self.param("hbadness")?,
                fuzz: self.param("hfuzz")?,
                overfull_rule: self.param("overfullrule")?,
            }
        } else {
            PackParams {
                badness: self.param("vbadness")?,
                fuzz: self.param("vfuzz")?,
                overfull_rule: 0,
            }
        })
    }

    /// `line_break` (§815) and `post_line_break` (§877–§890): the lines
    /// go to the enclosing vertical list.
    #[allow(clippy::too_many_lines)]
    fn line_break(&mut self, md: &Mode, items: &[ValueId]) -> R<()> {
        let v = self.v;
        let mut side = Side::default();
        let list: List = items.iter().filter_map(|&x| nodes::decode(v, x, &mut side)).collect();
        let view = self.font_view(&list)?;
        let shape = self.get("parshape")?.map(|x| list_of(v, x)).unwrap_or_default();
        let par_shape: Vec<(i32, i32)> = shape
            .chunks(2)
            .map(|c| {
                (
                    i32::try_from(int_of(v, c[0])).unwrap_or(0),
                    c.get(1).map_or(0, |&w| i32::try_from(int_of(v, w)).unwrap_or(0)),
                )
            })
            .collect();
        let packed = md.para.map_or(0, |_| 0);
        let _ = packed;
        let rec = self.get(&format!("par:{}", md.vlid))?.map(|x| list_of(v, x)).unwrap_or_default();
        let pg_packed = rec.get(2).map_or(0, |&x| int_of(v, x));
        let lang = phitex_typeset::linebreak::LangState {
            lang: u16::try_from(pg_packed % 65536).unwrap_or(0),
            lhm: u8::try_from((pg_packed / 65536) / 64).unwrap_or(2),
            rhm: u8::try_from((pg_packed / 65536) % 64).unwrap_or(3),
        };
        let params = phitex_typeset::linebreak::LineBreakParams {
            pretolerance: self.param("pretolerance")?,
            tolerance: self.param("tolerance")?,
            emergency_stretch: self.param("emergencystretch")?,
            looseness: self.param("looseness")?,
            line_penalty: self.param("linepenalty")?,
            hyphen_penalty: self.param("hyphenpenalty")?,
            ex_hyphen_penalty: self.param("exhyphenpenalty")?,
            adj_demerits: self.param("adjdemerits")?,
            double_hyphen_demerits: self.param("doublehyphendemerits")?,
            final_hyphen_demerits: self.param("finalhyphendemerits")?,
            left_skip: self.glue_param("leftskip")?,
            right_skip: self.glue_param("rightskip")?,
            par_fill_skip: self.glue_param("parfillskip")?,
            hsize: self.param("hsize")?,
            hang_indent: self.param("hangindent")?,
            hang_after: self.param("hangafter")?,
            par_shape,
            last_line_fit: 0,
            inter_line_penalty: self.param("interlinepenalty")?,
            club_penalty: self.param("clubpenalty")?,
            widow_penalty: self.param("widowpenalty")?,
            display_widow_penalty: self.param("displaywidowpenalty")?,
            broken_penalty: self.param("brokenpenalty")?,
            inter_line_penalties: Vec::new(),
            club_penalties: Vec::new(),
            widow_penalties: Vec::new(),
            display_widow_penalties: Vec::new(),
            prev_graf: self.int32(&format!("pg:{}", md.vlid))?,
            lang,
            pack: self.pack_params(true)?,
        };
        let patterns = self.patterns()?;
        let mut lc = [0u16; 256];
        for (c, x) in lc.iter_mut().enumerate() {
            *x = u16::try_from(self.int(&format!("lc:{c}"))?).unwrap_or(0);
        }
        let uc_hyph = self.param("uchyph")?;
        let hyphen = view.hyphen.clone();
        let lcf = |c: u16| lc.get(usize::from(c)).copied().unwrap_or(0);
        let hcf = |f: u32| hyphen.get(&f).copied().unwrap_or(-1);
        let mut hy = phitex_typeset::hyph::TexHyphenator {
            patterns: &patterns,
            lc_code: &lcf,
            uc_hyph,
            hyphen_char: &hcf,
        };
        let para = phitex_typeset::linebreak::line_break(list, false, &params, &view, &mut hy);
        let mut vm = md.clone();
        vm.lid = md.vlid;
        for line in para.lines {
            let b = nodes::encode_box(v, &line.packed.node, &side);
            self.append_to_vlist(&vm, b)?;
            for a in &line.packed.adjustments {
                let x = nodes::encode(v, a, &side);
                self.append(md.vlid, x);
            }
            if let Some(p) = line.penalty {
                let x = nodes::penalty_node(v, p);
                self.append(md.vlid, x);
            }
        }
        self.set(&format!("pg:{}", md.vlid), v.int(i64::from(para.prev_graf)), true);
        Ok(())
    }

    /// The hyphenation patterns and exceptions so far, entered once per
    /// value for the process.
    fn patterns(&mut self) -> R<Arc<phitex_typeset::hyph::Patterns>> {
        type Cache = Mutex<std::collections::HashMap<u64, Arc<phitex_typeset::hyph::Patterns>>>;
        static C: OnceLock<Cache> = OnceLock::new();
        let x = self.get("patterns")?;
        let key = x.map_or(0, ValueId::raw);
        let cache = C.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
        if let Some(p) = cache.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
            return Ok(p.clone());
        }
        let v = self.v;
        let mut p = phitex_typeset::hyph::Patterns::default();
        if let Some(x) = x {
            for e in list_of(v, x) {
                let xs = list_of(v, e);
                let lang = u16::try_from(xs.first().map_or(0, |&l| int_of(v, l))).unwrap_or(0);
                let kind = xs.get(1).map(|&k| crate::val::str_of(v, k)).unwrap_or_default();
                for w in xs.get(2).map(|&l| list_of(v, l)).unwrap_or_default() {
                    let word = crate::val::str_of(v, w);
                    if kind == "p" {
                        let _ = p.add_pattern_str(lang, &word, &|c| c);
                    } else {
                        let _ = p.add_exception_str(lang, &word, &|c| c);
                    }
                }
            }
        }
        let p = Arc::new(p);
        cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, p.clone());
        Ok(p)
    }

    // ------------------------------------------------------------ boxes

    /// The box in register `n` (`None`: void).
    pub(crate) fn box_reg(&mut self, n: i32) -> R<Option<ValueId>> {
        let x = self.get(&format!("box:{n}"))?;
        let v = self.v;
        Ok(x.and_then(|x| match &*v.get(x) {
            Value::List(xs) => xs.get(1).copied().filter(|&b| b != ValueId::UNIT),
            _ => None,
        }))
    }

    /// `eq_define` of a box register (§1077): its level is the group's.
    pub(crate) fn set_box_reg(&mut self, md: &Mode, n: i64, b: Option<ValueId>, global: bool) {
        let v = self.v;
        let lvl = if global { 0 } else { md.depth };
        let val = v.list(vec![v.int(lvl), b.unwrap_or(ValueId::UNIT)]);
        self.set(&format!("box:{n}"), val, global);
    }

    /// A box register changed "at the same level" (§1079 `box(n):=null`,
    /// §1247): the binding keeps its level; groups closing above that
    /// level define it again outside them (`lvlfix`).
    pub(crate) fn modify_box_reg(&mut self, n: i32, b: Option<ValueId>) -> R<()> {
        let v = self.v;
        let name = format!("box:{n}");
        let x = self.get(&name)?;
        let lvl = x.map_or(0, |x| crate::val::int_at(v, x, 0));
        let val = v.list(vec![v.int(lvl), b.unwrap_or(ValueId::UNIT)]);
        let md = self.mode()?;
        if lvl == 0 {
            self.set(&name, val, true);
        } else {
            self.set(&name, val, false);
            if lvl < md.depth {
                let mut fix = self.scope_list("lvlfix", &md)?;
                fix.push(v.str(&name));
                self.set_scope_list("lvlfix", &md, fix);
            }
        }
        Ok(())
    }

    /// `scan_box` (§1084).
    pub(crate) fn scan_box(&mut self, md: &Mode, ctx: i64) -> R<()> {
        self.skip_blanks_relax_x()?;
        if self.cur.cmd == cmd::MAKE_BOX {
            self.begin_box(md, ctx)
        } else if ctx >= LEADER_FLAG && (self.cur.cmd == cmd::HRULE || self.cur.cmd == cmd::VRULE) {
            let (w, h, d) = self.scan_rule_spec(self.cur.cmd == cmd::VRULE)?;
            let v = self.v;
            let r = v.list(vec![
                v.int(nodes::T_RULE),
                v.int(i64::from(w)),
                v.int(i64::from(h)),
                v.int(i64::from(d)),
            ]);
            self.box_end(md, ctx, Some(r))
        } else {
            // "A <box> was supposed to be here"
            self.back_cur();
            Ok(())
        }
    }

    /// `begin_box` (§1079–§1083).
    pub(crate) fn begin_box(&mut self, md: &Mode, ctx: i64) -> R<()> {
        let c = self.cur.chr;
        let v = self.v;
        let b: Option<ValueId> = match c {
            0 => {
                let n = self.scan_eight_bit_int()?;
                let b = self.box_reg(n)?;
                if b.is_some() {
                    self.modify_box_reg(n, None)?;
                }
                b
            }
            1 => {
                let n = self.scan_eight_bit_int()?;
                self.box_reg(n)?
            }
            2 => {
                // §1080
                if md.m.abs() == 203 {
                    None
                } else {
                    let (items, since) = self.list_items(md.lid)?;
                    let last = items.last().copied();
                    let ok = last.is_some_and(|x| nodes::tag(v, x) == nodes::T_BOX)
                        && !(md.m == 1 && since == 0);
                    if ok {
                        let m = nodes::marker(v, T_DELETE);
                        self.append(md.lid, m);
                        last.map(|x| {
                            let mut side = Side::default();
                            let mut bx = match nodes::decode(v, x, &mut side) {
                                Some(Node::Box(b)) => b,
                                _ => BoxNode::default(),
                            };
                            bx.shift = 0;
                            nodes::encode_box(v, &bx, &side)
                        })
                    } else {
                        None
                    }
                }
            }
            3 => {
                // §1082
                let n = self.scan_eight_bit_int()?;
                let _ = self.scan_keyword("to")?;
                let h = self.scan_normal_dimen()?;
                self.vsplit(n, h)?
            }
            _ => {
                // §1083
                let k = c - 4;
                let (grp, spec) = if k == 102 {
                    let g = if ctx < BOX_FLAG && md.m.abs() == 1 { group::ADJUSTED_HBOX } else { group::HBOX };
                    (g, self.scan_spec()?)
                } else if k == 1 {
                    (group::VBOX, self.scan_spec()?)
                } else {
                    (group::VTOP, self.scan_spec()?)
                };
                self.scan_left_brace()?;
                let sid = self.node_raw();
                let extra = vec![v.int(ctx), v.int(spec.0), v.int(i64::from(spec.1))];
                let m = if k == 102 { -102 } else { -1 };
                let kind = self.kind_value(md, grp, sid, sid, m, extra)?;
                self.open_scope_here(kind);
                let every = if k == 102 {
                    "param:everyhbox"
                } else {
                    self.normal_paragraph()?;
                    "param:everyvbox"
                };
                let e = self.get(every)?;
                let toks = crate::val::toks_of(v, e);
                self.push_toks(toks);
                return Ok(());
            }
        };
        self.box_end(md, ctx, b)
    }

    /// `scan_spec` (§645): `(0 exactly | 1 additional, size)`.
    fn scan_spec(&mut self) -> R<(i64, i32)> {
        if self.scan_keyword("to")? {
            Ok((0, self.scan_normal_dimen()?))
        } else if self.scan_keyword("spread")? {
            Ok((1, self.scan_normal_dimen()?))
        } else {
            Ok((1, 0))
        }
    }

    /// `box_end` (§1075–§1078).
    pub(crate) fn box_end(&mut self, md: &Mode, ctx: i64, b: Option<ValueId>) -> R<()> {
        let v = self.v;
        if ctx < BOX_FLAG {
            // §1076
            let Some(b) = b else { return Ok(()) };
            let b = with_shift(v, b, i32::try_from(ctx).unwrap_or(0));
            if md.m.abs() == 1 {
                self.append_to_vlist(md, b)?;
                if md.m > 0 {
                    self.build_page_mark(md)?;
                }
            } else {
                let b = if md.m.abs() == 102 {
                    self.set_space_factor(md, 1000);
                    b
                } else {
                    crate::math::box_noad(v, b)
                };
                self.append(md.lid, b);
            }
        } else if ctx < SHIP_OUT_FLAG {
            // §1077
            let (n, global) = if ctx < BOX_FLAG + 256 { (ctx - BOX_FLAG, false) } else { (ctx - BOX_FLAG - 256, true) };
            self.set_box_reg(md, n, b, global);
        } else if let Some(b) = b {
            if ctx > SHIP_OUT_FLAG {
                // §1078
                self.skip_blanks_relax_x()?;
                if (self.cur.cmd == cmd::HSKIP && md.m.abs() != 1) || (self.cur.cmd == cmd::VSKIP && md.m.abs() == 1) {
                    let g = self.glue_of_cur()?;
                    let kind = match ctx - LEADER_FLAG {
                        0 => GlueKind::ALeaders,
                        1 => GlueKind::CLeaders,
                        _ => GlueKind::XLeaders,
                    };
                    let mut side = Side::default();
                    let leader = nodes::decode(v, b, &mut side).map(Box::new);
                    let node = Node::Glue(phitex_typeset::node::GlueNode { spec: g, kind, leader });
                    let x = nodes::encode(v, &node, &side);
                    self.append(md.lid, x);
                } else {
                    // "Leaders not followed by proper glue"
                    self.back_cur();
                }
            } else {
                self.ship_out(b)?;
            }
        }
        Ok(())
    }

    /// The glue of the `\hskip`-like command in `cur` (§1060).
    fn glue_of_cur(&mut self) -> R<Glue> {
        Ok(match self.cur.chr {
            0 => fil(1, 0),
            1 => fil(2, 0),
            2 => Glue {
                stretch: 65536,
                stretch_order: phitex_typeset::glue::Order::Fil,
                shrink: 65536,
                shrink_order: phitex_typeset::glue::Order::Fil,
                ..Glue::ZERO
            },
            3 => Glue {
                stretch: -65536,
                stretch_order: phitex_typeset::glue::Order::Fil,
                ..Glue::ZERO
            },
            4 => self.scan_glue(2)?,
            _ => self.scan_glue(3)?,
        })
    }

    /// `append_glue` (§1060).
    pub(crate) fn append_glue(&mut self, md: &Mode) -> R<()> {
        let s = self.cur.chr;
        let g = self.glue_of_cur()?;
        let kind = if s > 4 { GlueKind::Mu } else { GlueKind::Normal };
        let n = nodes::glue_node(self.v, &g, kind);
        self.append(md.lid, n);
        Ok(())
    }

    /// `delete_last` (§1105).
    pub(crate) fn delete_last(&mut self, md: &Mode) -> R<()> {
        let want = match self.cur.chr {
            10 => nodes::T_GLUE,
            11 => nodes::T_KERN,
            _ => nodes::T_PENALTY,
        };
        let (items, since) = self.list_items(md.lid)?;
        if md.m == 1 && since == 0 {
            // §1106: nothing can be taken from the current page
            return Ok(());
        }
        if items.last().is_some_and(|&x| nodes::tag(self.v, x) == want) {
            let m = nodes::marker(self.v, T_DELETE);
            self.append(md.lid, m);
        }
        Ok(())
    }

    /// `unpackage` (§1110).
    pub(crate) fn unpackage(&mut self, md: &Mode) -> R<()> {
        let c = self.cur.chr;
        let n = self.scan_eight_bit_int()?;
        let Some(b) = self.box_reg(n)? else { return Ok(()) };
        let v = self.v;
        let h = nodes::box_is_h(v, b);
        if md.m.abs() == 203 || (md.m.abs() == 1 && h) || (md.m.abs() == 102 && !h) {
            // "Incompatible list can't be unboxed"
            return Ok(());
        }
        let xs = list_of(v, b);
        for x in xs.get(9).map(|&l| list_of(v, l)).unwrap_or_default() {
            self.append(md.lid, x);
        }
        if c == 0 {
            self.modify_box_reg(n, None)?;
        }
        Ok(())
    }

    /// `append_italic_correction` (§1113).
    pub(crate) fn append_italic_correction(&mut self, md: &Mode) -> R<()> {
        let (items, _) = self.list_items(md.lid)?;
        let Some(&last) = items.last() else { return Ok(()) };
        let v = self.v;
        let mut side = Side::default();
        let (f, c) = match nodes::decode(v, last, &mut side) {
            Some(Node::Char { font, ch }) => (font, ch),
            Some(Node::Lig(l)) => (l.font, l.ch),
            _ => return Ok(()),
        };
        let mut view = crate::fonts::View::default();
        self.add_font(&mut view, f)?;
        let ic = view.char_dims(f, u32::from(c)).map_or(0, |d| d.italic);
        let k = nodes::kern_node(v, ic, KernKind::Explicit);
        self.append(md.lid, k);
        Ok(())
    }

    /// `append_discretionary` (§1117).
    pub(crate) fn append_discretionary(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        if self.cur.chr == 1 {
            let f = self.int("font")?;
            let c = self.int32(&format!("hc:{f}"))?;
            let mut d = Disc::default();
            if (0..256).contains(&c) {
                let mut view = crate::fonts::View::default();
                self.add_font(&mut view, u32::try_from(f).unwrap_or(0))?;
                if view.exists(u32::try_from(f).unwrap_or(0), u32::try_from(c).unwrap_or(0)) {
                    d.pre.push(Node::Char {
                        font: u32::try_from(f).unwrap_or(0),
                        ch: u16::try_from(c).unwrap_or(0),
                    });
                }
            }
            let x = nodes::encode(v, &Node::Disc(d), &Side::default());
            self.append(md.lid, x);
        } else {
            self.scan_left_brace()?;
            let sid = self.node_raw();
            let extra = vec![v.int(0), v.int(md.lid), v.list(Vec::new()), v.list(Vec::new())];
            let kind = self.kind_value(md, group::DISC, sid, sid, -102, extra)?;
            self.open_scope_here(kind);
        }
        Ok(())
    }

    /// `make_accent` (§1123–§1125).
    pub(crate) fn make_accent(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let a_ch = self.scan_char_num()?;
        let f = u32::try_from(self.int("font")?).unwrap_or(0);
        let mut view = crate::fonts::View::default();
        self.add_font(&mut view, f)?;
        if !view.exists(f, u32::try_from(a_ch).unwrap_or(0)) {
            return Ok(());
        }
        let p_char = Node::Char {
            font: f,
            ch: u16::try_from(a_ch).unwrap_or(0),
        };
        let x = self.font_dimen(i64::from(f), 5)?;
        let slant = self.font_dimen(i64::from(f), 1)?;
        let s = f64::from(slant) / 65536.0;
        let a = view.char_dims(f, u32::try_from(a_ch).unwrap_or(0)).map_or(0, |d| d.width);
        self.do_assignments()?;
        // §1124
        let f2 = u32::try_from(self.int("font")?).unwrap_or(0);
        let mut view2 = crate::fonts::View::default();
        self.add_font(&mut view2, f2)?;
        let q: Option<u32> = match self.cur.cmd {
            cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN => Some(u32::try_from(self.cur.chr).unwrap_or(0)),
            cmd::CHAR_NUM => Some(u32::try_from(self.scan_char_num()?).unwrap_or(0)),
            _ => {
                self.back_cur();
                None
            }
        };
        let q = q.filter(|&c| view2.exists(f2, c));
        let mut out: Vec<Node> = Vec::new();
        match q {
            Some(qc) => {
                // §1125
                let t = f64::from(self.font_dimen(i64::from(f2), 1)?) / 65536.0;
                let d = view2.char_dims(f2, qc).unwrap_or_default();
                let (w, h) = (d.width, d.height);
                let mut p = p_char;
                if h != x {
                    let pp = phitex_typeset::pack::hpack(vec![p], Spec::Additional(0), &view, &PackParams::default(), false);
                    let mut bx = pp.node;
                    bx.shift = x - h;
                    p = Node::Box(bx);
                }
                let delta = (f64::from(w - a) / 2.0 + f64::from(h) * t - f64::from(x) * s).round();
                #[allow(clippy::cast_possible_truncation)]
                let delta = delta as i32;
                out.push(Node::Kern(delta, KernKind::AccKern));
                out.push(p);
                out.push(Node::Kern(-a - delta, KernKind::AccKern));
                out.push(Node::Char {
                    font: f2,
                    ch: u16::try_from(qc).unwrap_or(0),
                });
            }
            None => out.push(p_char),
        }
        let side = Side::default();
        for n in &out {
            let x = nodes::encode(v, n, &side);
            self.append(md.lid, x);
        }
        self.set_space_factor(md, 1000);
        Ok(())
    }

    /// `do_assignments` (§1270).
    pub(crate) fn do_assignments(&mut self) -> R<()> {
        loop {
            self.skip_blanks_relax_x()?;
            if self.cur.cmd <= cmd::MAX_NON_PREFIXED {
                return Ok(());
            }
            self.prefixed_command()?;
        }
    }

    /// `begin_insert_or_adjust` (§1099).
    pub(crate) fn begin_insert_or_adjust(&mut self, md: &Mode, adjust: bool) -> R<()> {
        let n = if adjust {
            255
        } else {
            let n = self.scan_eight_bit_int()?;
            if n == 255 { 0 } else { n }
        };
        self.scan_left_brace()?;
        let v = self.v;
        let sid = self.node_raw();
        let kind = self.kind_value(md, group::INSERT, sid, sid, -1, vec![v.int(i64::from(n)), v.int(md.lid)])?;
        self.open_scope_here(kind);
        self.normal_paragraph()?;
        Ok(())
    }

    /// `handle_right_brace` (§1068, §1085, §1100, §1118).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn handle_right_brace(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        // a paragraph inside a vertical group ends first (`end_graf`)
        if md.m == 102 && matches!(md.group, group::VBOX | group::VTOP | group::INSERT | group::OUTPUT | group::VCENTER) {
            let t = self.cur.tok.clone();
            self.back_input(t);
            self.back_input(Tok::cs("\u{10FFFE}endgraf"));
            return Ok(());
        }
        match md.group {
            group::SIMPLE => self.close_simple(md),
            group::HBOX | group::ADJUSTED_HBOX | group::VBOX | group::VTOP => {
                // §1086 `package`
                self.effect = Effect::Close;
                let x = |i: usize| md.extra.get(i).map_or(0, |&y| int_of(v, y));
                let (ctx, spec_code, size) = (x(0), x(1), i32::try_from(x(2)).unwrap_or(0));
                let spec = if spec_code == 0 { Spec::Exactly(size) } else { Spec::Additional(size) };
                let d = self.param("boxmaxdepth")?;
                let (items, _) = self.list_items(md.sid)?;
                let mut side = Side::default();
                let list: List = items.iter().filter_map(|&y| nodes::decode(v, y, &mut side)).collect();
                let adjusted = md.group == group::ADJUSTED_HBOX;
                let packed = if md.group == group::HBOX || adjusted {
                    let view = self.font_view(&list)?;
                    let pp = self.pack_params(true)?;
                    phitex_typeset::pack::hpack(list, spec, &view, &pp, adjusted)
                } else {
                    let pp = self.pack_params(false)?;
                    let mut p = phitex_typeset::pack::vpackage(list, spec, d, &pp);
                    if md.group == group::VTOP {
                        // §1087
                        let h = match p.node.list.first() {
                            Some(n @ (Node::Box(_) | Node::Rule(_))) => n.box_dims().map_or(0, |b| b.1),
                            _ => 0,
                        };
                        p.node.depth = p.node.depth - h + p.node.height;
                        p.node.height = h;
                    }
                    p
                };
                self.set("badness", v.int(i64::from(packed.badness)), true);
                self.unsave_extras(md)?;
                let b = nodes::encode_box(v, &packed.node, &side);
                let outer = self.outer_mode(md)?;
                if ctx >= BOX_FLAG && ctx < SHIP_OUT_FLAG && ctx < BOX_FLAG + 256 {
                    // a local \setbox lands outside the group
                    let lvl = v.int(outer.depth);
                    let val = v.list(vec![lvl, b]);
                    let def = v.list(vec![v.str(&format!("box:{}", ctx - BOX_FLAG)), val, v.int(0)]);
                    let put = self.ops.put;
                    self.kid(put, vec![v.list(vec![def]), v.list(Vec::new())]);
                    let _ = SHIP_OUT_FLAG;
                } else {
                    self.box_end(&outer, ctx, Some(b))?;
                }
                if adjusted {
                    for a in &packed.adjustments {
                        let y = nodes::encode(v, a, &side);
                        self.append(outer.vlid, y);
                    }
                }
                Ok(())
            }
            group::INSERT => {
                // §1100
                self.effect = Effect::Close;
                let n = md.extra.first().map_or(0, |&y| int_of(v, y));
                let q = self.glue_param("splittopskip")?;
                let d = self.param("splitmaxdepth")?;
                let f = self.param("floatingpenalty")?;
                let (items, _) = self.list_items(md.sid)?;
                let mut side = Side::default();
                let list: List = items.iter().filter_map(|&y| nodes::decode(v, y, &mut side)).collect();
                let p = phitex_typeset::pack::vpack(list, Spec::Additional(0), &PackParams::default());
                self.unsave_extras(md)?;
                let outer = self.outer_mode(md)?;
                let node = if n < 255 {
                    Node::Ins(phitex_typeset::node::Insert {
                        number: u16::try_from(n).unwrap_or(0),
                        height: p.node.height + p.node.depth,
                        depth: d,
                        split_top_skip: q,
                        float_cost: f,
                        list: p.node.list,
                    })
                } else {
                    Node::Adjust(p.node.list)
                };
                let y = nodes::encode(v, &node, &side);
                self.append(outer.lid, y);
                if outer.lid == 0 && outer.m == 1 {
                    self.build_page_mark(&outer)?;
                }
                Ok(())
            }
            group::DISC => self.build_discretionary(md),
            group::OUTPUT => crate::page::output_end(self, md),
            group::MATH => self.math_group_end(md),
            group::VCENTER => self.vcenter_end(md),
            group::MATH_CHOICE => self.build_choices(md),
            // "Extra }, or forgotten $" / "\right" (§1069)
            group::MATH_SHIFT | group::MATH_LEFT | group::SEMI_SIMPLE => Ok(()),
            _ => Ok(()),
        }
    }

    /// The mode outside the innermost group (what `pop_nest` returns to).
    pub(crate) fn outer_mode(&mut self, md: &Mode) -> R<Mode> {
        // The enclosing scope's kind is the one the opener read; it is
        // kept in the group's own kind as the parent list.
        let v = self.v;
        let parent = md.extra.last().copied();
        let _ = parent;
        let saved = self.kind_outer(md)?;
        let mut o = saved;
        if o.km.abs() == 1
            && let Some(p) = self.get(&format!("par:{}", o.lid))?
            && p != ValueId::UNIT
        {
            let xs = list_of(v, p);
            let id = xs.first().map_or(0, |&x| int_of(v, x));
            let lang = xs.get(1).map_or(0, |&x| int_of(v, x));
            o.m = 102;
            o.lid = id;
            o.para = Some((id, lang));
        }
        Ok(o)
    }

    fn kind_outer(&mut self, _md: &Mode) -> R<Mode> {
        let v = self.v;
        let cur = self.scope_kind()?.unwrap_or(ValueId::UNIT);
        let k = list_of(v, cur).get(5).copied().unwrap_or(ValueId::UNIT);
        if k == ValueId::UNIT {
            return Ok(Mode {
                m: 1,
                km: 1,
                lid: 0,
                vlid: 0,
                group: 0,
                sid: 0,
                depth: 0,
                extra: Vec::new(),
                para: None,
                vm: 1,
            });
        }
        let xs = list_of(v, k);
        let n = |i: usize| xs.get(i).map_or(0, |&y| int_of(v, y));
        let km = i32::try_from(n(3)).unwrap_or(1);
        Ok(Mode {
            m: km,
            km,
            lid: n(2),
            vlid: n(2),
            group: n(0),
            sid: n(1),
            depth: n(4),
            extra: xs.get(6..).map(<[ValueId]>::to_vec).unwrap_or_default(),
            para: None,
            vm: km,
        })
    }

    /// `build_discretionary` (§1119–§1121).
    fn build_discretionary(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let part = md.extra.first().map_or(0, |&y| int_of(v, y));
        let outer_lid = md.extra.get(1).map_or(0, |&y| int_of(v, y));
        let (items, _) = self.list_items(md.sid)?;
        let mut side = Side::default();
        let mut list: List = Vec::new();
        for &y in &items {
            if let Some(n) = nodes::decode(v, y, &mut side) {
                match n {
                    Node::Char { .. } | Node::Kern(..) | Node::Box(_) | Node::Rule(_) | Node::Lig(_) => list.push(n),
                    _ => break, // "Improper discretionary list"
                }
            }
        }
        let enc = nodes::encode_list(v, &list, &side);
        self.unsave_extras(md)?;
        if part < 2 {
            let mut extra = md.extra.clone();
            extra[0] = v.int(part + 1);
            if part == 0 {
                extra[2] = enc;
            } else {
                extra[3] = enc;
            }
            self.scan_left_brace()?;
            let sid = self.node_raw();
            let mut outer = md.clone();
            outer.depth -= 1;
            let cur = self.scope_kind()?.unwrap_or(ValueId::UNIT);
            let parent = list_of(v, cur).get(5).copied().unwrap_or(ValueId::UNIT);
            let kind = self.kind_with_parent(&outer, group::DISC, sid, sid, -102, extra, parent);
            self.effect = Effect::Scope(Brackets {
                closes: 1,
                opens: 1,
                before: false,
                kind: Some(kind),
            });
            return Ok(());
        }
        self.effect = Effect::Close;
        let mut s2 = Side::default();
        let pre = md.extra.get(2).map_or_else(Vec::new, |&l| nodes::decode_list(v, l, &mut s2));
        let post = md.extra.get(3).map_or_else(Vec::new, |&l| nodes::decode_list(v, l, &mut s2));
        let d = Node::Disc(Disc {
            pre,
            post,
            replace: list,
        });
        // the replacement list follows the disc node in tex.web's list;
        // here it is inside it
        let y = nodes::encode(v, &d, &merge_sides(s2, side));
        self.append(outer_lid, y);
        Ok(())
    }

    // ------------------------------------------------------- characters

    /// Characters and spaces (§1030–§1044): `main_loop` and the spaces
    /// after it, while the input lasts without claiming more.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn char_run(&mut self, md: &Mode, cancel: bool) -> R<()> {
        let v = self.v;
        let f = u32::try_from(self.int("font")?).unwrap_or(0);
        let mut view = crate::fonts::View::default();
        self.add_font(&mut view, f)?;
        let mut sf = self.space_factor(md)?;
        let sf0 = sf;
        let mut out: Vec<Node> = Vec::new();
        let mut cancel_boundary = cancel;
        loop {
            // reswitch
            let c = self.cur.cmd;
            match c {
                cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN | cmd::CHAR_NUM => {
                    let ch = if c == cmd::CHAR_NUM {
                        self.scan_char_num()?
                    } else {
                        self.cur.chr
                    };
                    let next = self.main_loop(md, f, &view, u32::try_from(ch).unwrap_or(0), &mut sf, &mut out, &mut cancel_boundary)?;
                    if next {
                        continue;
                    }
                }
                cmd::NO_BOUNDARY => {
                    self.get_x_token()?;
                    if matches!(self.cur.cmd, cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN | cmd::CHAR_NUM) {
                        cancel_boundary = true;
                    }
                    continue;
                }
                cmd::SPACER | cmd::EX_SPACE => {
                    let normal = c == cmd::EX_SPACE || sf == 1000;
                    let g = self.space_glue(f, sf, normal)?;
                    out.push(g);
                }
                _ => {
                    self.back_cur();
                    break;
                }
            }
            // big_switch
            if !self.has_input() {
                break;
            }
            self.get_x_token()?;
            if !matches!(
                self.cur.cmd,
                cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN | cmd::CHAR_NUM | cmd::NO_BOUNDARY | cmd::SPACER | cmd::EX_SPACE
            ) {
                self.back_cur();
                break;
            }
        }
        let side = Side::default();
        for n in &out {
            let x = nodes::encode(v, n, &side);
            self.append(md.lid, x);
        }
        if sf != sf0 {
            self.set_space_factor(md, sf);
        }
        Ok(())
    }

    /// A space (§1041–§1044).
    pub(crate) fn space_glue(&mut self, f: u32, sf: i32, normal: bool) -> R<Node> {
        let ss = self.glue_param("spaceskip")?;
        let zero = |g: &Glue| g.width == 0 && g.stretch == 0 && g.shrink == 0;
        let fi = i64::from(f);
        let font_glue = |r: &mut Self| -> R<Glue> {
            Ok(Glue::new(r.font_dimen(fi, 2)?, r.font_dimen(fi, 3)?, r.font_dimen(fi, 4)?))
        };
        if normal {
            return Ok(if zero(&ss) {
                Node::glue(font_glue(self)?)
            } else {
                Node::param_glue(12, ss)
            });
        }
        let xs = self.glue_param("xspaceskip")?;
        if sf >= 2000 && !zero(&xs) {
            return Ok(Node::param_glue(13, xs));
        }
        let mut g = if zero(&ss) { font_glue(self)? } else { ss };
        if sf >= 2000 {
            g.width += self.font_dimen(fi, 7)?;
        }
        g.stretch = phitex_typeset::arith::xn_over_d(g.stretch, sf, 1000).value;
        g.shrink = phitex_typeset::arith::xn_over_d(g.shrink, 1000, sf).value;
        Ok(Node::glue(g))
    }

    /// `adjust_space_factor` (§1034).
    fn adjust_sf(&mut self, ch: u32, sf: &mut i32) -> R<()> {
        let s = self.int32(&format!("sf:{ch}"))?;
        if s == 1000 {
            *sf = 1000;
        } else if s < 1000 {
            if s > 0 {
                *sf = s;
            }
        } else if *sf < 1000 {
            *sf = 1000;
        } else {
            *sf = s;
        }
        Ok(())
    }

    /// `main_loop` (§1034–§1040) from character `ch`: true when the
    /// token after the characters is in `cur` (`goto reswitch`), false
    /// at `goto big_switch` (or at the end of the node's text).
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn main_loop(
        &mut self,
        md: &Mode,
        f: u32,
        view: &crate::fonts::View,
        ch: u32,
        sf: &mut i32,
        out: &mut Vec<Node>,
        cancel_boundary: &mut bool,
    ) -> R<bool> {
        #[derive(Clone, Copy, PartialEq)]
        enum L {
            Wrapup,
            Move,
            Move1,
            Move2,
            Lookahead,
            Lig,
            MoveLig,
        }
        #[derive(Clone, Copy)]
        struct Item {
            ch: u32,
            lig: bool,
            ptr: Option<u32>,
        }
        let Some((loaded, _)) = view.fonts.get(&f).cloned() else {
            // the null font: every character is missing
            return Ok(false);
        };
        let tfm = &loaded.tfm;
        let mut bchar: Option<u32> = tfm.bchar().map(u32::from);
        let false_bchar: Option<u32> = tfm.false_bchar().map(u32::from);
        let hyphen_char = view.hyphen.get(&f).copied().unwrap_or(-1);
        let para = md.m > 0;
        self.adjust_sf(ch, sf)?;
        let mut lig_stack: Vec<Item> = vec![Item {
            ch,
            lig: false,
            ptr: None,
        }];
        let mut cur_l: Option<u32> = Some(ch);
        let mut cur_r: Option<u32> = None;
        let mut cur_q = out.len();
        let mut lig_present = false;
        let mut lft_hit = false;
        let mut rt_hit = false;
        let mut ins_disc = false;
        let mut input_done = false;
        let lookup = |l: Option<u32>, r: Option<u32>| -> Option<phitex_tfm::LigKernStep> {
            let r = u8::try_from(r?).ok()?;
            let l = match l {
                Some(c) => Some(u8::try_from(c).ok()?),
                None => None,
            };
            tfm.lookup(l, Some(r))
        };
        let wrapup = |z: bool,
                      cur_l: Option<u32>,
                      cur_q: usize,
                      out: &mut Vec<Node>,
                      lig_present: &mut bool,
                      lft_hit: &mut bool,
                      rt_hit: &mut bool,
                      ins_disc: &mut bool,
                      stack_empty: bool| {
            // §1035
            let Some(cl) = cur_l else { return };
            if out.len() > cur_q
                && matches!(out.last(), Some(Node::Char { ch, .. }) if i32::from(*ch) == hyphen_char)
            {
                *ins_disc = true;
            }
            if *lig_present {
                let original: Vec<u16> = out[cur_q..]
                    .iter()
                    .filter_map(|n| match n {
                        Node::Char { ch, .. } => Some(*ch),
                        _ => None,
                    })
                    .collect();
                let mut lg = Ligature {
                    font: f,
                    ch: u16::try_from(cl).unwrap_or(0),
                    original,
                    left_hit: false,
                    right_hit: false,
                };
                if *lft_hit {
                    lg.left_hit = true;
                    *lft_hit = false;
                }
                if z && stack_empty {
                    lg.right_hit = true;
                    *rt_hit = false;
                }
                out.truncate(cur_q);
                out.push(Node::Lig(lg));
                *lig_present = false;
            }
            if *ins_disc {
                *ins_disc = false;
                if para {
                    out.push(Node::Disc(Disc::default()));
                }
            }
        };
        let mut at = if *cancel_boundary {
            *cancel_boundary = false;
            L::Move2
        } else if tfm.bchar_label().is_some() {
            cur_r = cur_l;
            cur_l = None;
            L::Lig
        } else {
            L::Move2
        };
        loop {
            match at {
                L::Wrapup => {
                    wrapup(
                        rt_hit,
                        cur_l,
                        cur_q,
                        out,
                        &mut lig_present,
                        &mut lft_hit,
                        &mut rt_hit,
                        &mut ins_disc,
                        lig_stack.is_empty(),
                    );
                    at = L::Move;
                }
                L::Move => {
                    // §1036
                    let Some(top) = lig_stack.last().copied() else {
                        return Ok(!input_done);
                    };
                    cur_q = out.len();
                    cur_l = Some(top.ch);
                    at = L::Move1;
                }
                L::Move1 => {
                    at = if lig_stack.last().is_some_and(|t| t.lig) { L::MoveLig } else { L::Move2 };
                }
                L::Move2 => {
                    let cl = cur_l.unwrap_or(0);
                    if !view.exists(f, cl) {
                        // char_warning (§581)
                        lig_stack.pop();
                        self.char_warning(f, cl)?;
                        return Ok(false);
                    }
                    out.push(Node::Char {
                        font: f,
                        ch: u16::try_from(cl).unwrap_or(0),
                    });
                    lig_stack.pop();
                    at = L::Lookahead;
                }
                L::Lookahead => {
                    // §1038; at the end of the node's text the next token
                    // is not a character (characters are one node)
                    let mut got: Option<u32> = None;
                    if !self.has_input() {
                        input_done = true;
                    } else if self.get_next()? {
                        if !matches!(self.cur.cmd, cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN) {
                            self.x_token()?;
                        }
                        match self.cur.cmd {
                            cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN => {
                                got = Some(u32::try_from(self.cur.chr).unwrap_or(0));
                            }
                            cmd::CHAR_NUM => {
                                got = Some(u32::try_from(self.scan_char_num()?).unwrap_or(0));
                            }
                            cmd::NO_BOUNDARY => bchar = None,
                            _ => {}
                        }
                    } else {
                        self.get_next_or_end()?;
                    }
                    match got {
                        Some(c) => {
                            self.adjust_sf(c, sf)?;
                            lig_stack.push(Item {
                                ch: c,
                                lig: false,
                                ptr: None,
                            });
                            cur_r = Some(c);
                            if cur_r == false_bchar {
                                cur_r = None;
                            }
                        }
                        None => {
                            cur_r = bchar;
                            lig_stack.clear();
                        }
                    }
                    at = L::Lig;
                }
                L::MoveLig => {
                    // §1037
                    let top = lig_stack.pop().unwrap_or(Item {
                        ch: 0,
                        lig: true,
                        ptr: None,
                    });
                    if let Some(c) = top.ptr {
                        out.push(Node::Char {
                            font: f,
                            ch: u16::try_from(c).unwrap_or(0),
                        });
                    }
                    lig_present = true;
                    if let Some(t) = lig_stack.last() {
                        cur_r = Some(t.ch);
                        at = L::Lig;
                    } else if top.ptr.is_some() {
                        at = L::Lookahead;
                    } else {
                        cur_r = bchar;
                        at = L::Lig;
                    }
                }
                L::Lig => {
                    // §1039–§1040
                    let Some(step) = lookup(cur_l, cur_r) else {
                        at = L::Wrapup;
                        continue;
                    };
                    if step.is_kern() {
                        wrapup(
                            rt_hit,
                            cur_l,
                            cur_q,
                            out,
                            &mut lig_present,
                            &mut lft_hit,
                            &mut rt_hit,
                            &mut ins_disc,
                            lig_stack.is_empty(),
                        );
                        out.push(Node::Kern(loaded.at.char_kern(step), KernKind::Normal));
                        at = L::Move;
                        continue;
                    }
                    if cur_l.is_none() {
                        lft_hit = true;
                    } else if lig_stack.is_empty() {
                        rt_hit = true;
                    }
                    let rem = u32::from(step.remainder);
                    match step.op {
                        1 | 5 => {
                            cur_l = Some(rem);
                            lig_present = true;
                        }
                        2 | 6 => {
                            cur_r = Some(rem);
                            match lig_stack.last_mut() {
                                None => {
                                    lig_stack.push(Item {
                                        ch: rem,
                                        lig: true,
                                        ptr: None,
                                    });
                                    bchar = None;
                                }
                                Some(t) if !t.lig => {
                                    let c = t.ch;
                                    *t = Item {
                                        ch: rem,
                                        lig: true,
                                        ptr: Some(c),
                                    };
                                }
                                Some(t) => t.ch = rem,
                            }
                        }
                        3 => {
                            cur_r = Some(rem);
                            lig_stack.push(Item {
                                ch: rem,
                                lig: true,
                                ptr: None,
                            });
                        }
                        7 | 11 => {
                            wrapup(
                                false,
                                cur_l,
                                cur_q,
                                out,
                                &mut lig_present,
                                &mut lft_hit,
                                &mut rt_hit,
                                &mut ins_disc,
                                lig_stack.is_empty(),
                            );
                            cur_q = out.len();
                            cur_l = Some(rem);
                            lig_present = true;
                        }
                        _ => {
                            cur_l = Some(rem);
                            lig_present = true;
                            at = if lig_stack.is_empty() { L::Wrapup } else { L::Move1 };
                            continue;
                        }
                    }
                    if step.op > 4 && step.op != 7 {
                        at = L::Wrapup;
                        continue;
                    }
                    at = L::Lig;
                }
            }
        }
    }

    /// `char_warning` (§581).
    fn char_warning(&mut self, f: u32, c: u32) -> R<()> {
        if self.param("tracinglostchars")? > 0 {
            let mut s = String::from("Missing character: There is no ");
            crate::print::push_code(&mut s, c);
            s.push_str(" in font ");
            let info = self.get(&format!("fontinfo:{f}"))?;
            if let Some(i) = crate::fonts::info(self.v, info) {
                s.push_str(&i.name);
            }
            s.push('!');
            self.log("diag", &s)?;
        }
        Ok(())
    }

    /// `\lastpenalty`, `\lastkern`, `\lastskip`, `\inputlineno`,
    /// `\badness` (§424).
    pub(crate) fn last_item(&mut self, m: i32) -> R<crate::scan::IVal> {
        use crate::scan::IVal;
        if m > 2 {
            return Ok(IVal::Int(if m == 3 {
                self.input_line_no()?
            } else {
                self.int32("badness")?
            }));
        }
        let md = self.mode()?;
        let (items, _) = self.list_items(md.lid)?;
        let v = self.v;
        let last = items.last().copied();
        let tag = last.map_or(-1, |x| nodes::tag(v, x));
        Ok(match m {
            0 => IVal::Int(if tag == nodes::T_PENALTY { nodes::int_field(v, last.unwrap_or(ValueId::UNIT), 1) } else { 0 }),
            1 => IVal::Dimen(if tag == nodes::T_KERN { nodes::int_field(v, last.unwrap_or(ValueId::UNIT), 1) } else { 0 }),
            _ => {
                if tag == nodes::T_GLUE {
                    let x = last.unwrap_or(ValueId::UNIT);
                    let g = nodes::glue_of_node(v, x);
                    if nodes::int_field(v, x, 2) == 2 { IVal::Mu(g) } else { IVal::Glue(g) }
                } else {
                    IVal::Glue(Glue::ZERO)
                }
            }
        })
    }

    /// `its_all_over` (§1054): the end, once the page and its
    /// contributions are empty and no output routine is pending; else a
    /// last page is forced out and `\end` read again.
    pub(crate) fn its_all_over(&mut self, md: &Mode) -> R<()> {
        if md.m < 0 {
            return Ok(());
        }
        if self.page_empty()? && self.int("deadcycles")? == 0 {
            // nothing after `\end` is read
            self.claim_rest = true;
            self.frames.clear();
            return Ok(());
        }
        self.back_input(Tok::cs("end"));
        let v = self.v;
        let w = self.param("hsize")?;
        let b = null_box(v, w);
        self.append(0, b);
        let fill = Glue {
            stretch: 65536,
            stretch_order: phitex_typeset::glue::Order::Fill,
            ..Glue::ZERO
        };
        let g = nodes::glue_node(v, &fill, GlueKind::Normal);
        self.append(0, g);
        let p = nodes::penalty_node(v, -0o10000000000);
        self.append(0, p);
        self.build_page(md)
    }

    /// `\vsplit` (§977): the box split off register `n` at height `h`.
    fn vsplit(&mut self, n: i32, h: i32) -> R<Option<ValueId>> {
        let Some(b) = self.box_reg(n)? else { return Ok(None) };
        let v = self.v;
        let mut side = Side::default();
        let Some(Node::Box(bx)) = nodes::decode(v, b, &mut side) else {
            return Ok(None);
        };
        if bx.kind != BoxKind::V {
            return Ok(None);
        }
        let stk = self.glue_param("splittopskip")?;
        let smd = self.param("splitmaxdepth")?;
        let pp = self.pack_params(false)?;
        let r = phitex_typeset::page::vsplit(bx.list, h, smd, stk, false, &pp);
        let marks: Vec<ValueId> = r.marks.iter().filter_map(|&k| side.marks.get(k as usize).copied()).collect();
        if let Some(&first) = marks.first() {
            self.set("mark:splitfirst", first, true);
            self.set("mark:splitbot", *marks.last().unwrap_or(&first), true);
        }
        let rest = r.rest.map(|rb| nodes::encode_box(v, &rb, &side));
        self.modify_box_reg(n, rest)?;
        Ok(Some(nodes::encode_box(v, &r.page.node, &side)))
    }
}

fn fil(order: i64, _w: i32) -> Glue {
    Glue {
        stretch: 65536,
        stretch_order: crate::val::order(order),
        ..Glue::ZERO
    }
}

fn with_shift(v: &phitex_defuse::Values, b: ValueId, s: i32) -> ValueId {
    match &*v.get(b) {
        Value::List(xs) if nodes::tag(v, b) == nodes::T_BOX => {
            let mut ys = xs.to_vec();
            ys[5] = v.int(i64::from(s));
            v.list(ys)
        }
        _ => b,
    }
}

fn merge_sides(a: Side, b: Side) -> Side {
    let mut s = a;
    let off_m = s.marks.len();
    let _ = off_m;
    s.marks.extend(b.marks);
    s.whats.extend(b.whats);
    s
}

pub(crate) fn _unused(_: ValueId) -> Option<Glue> {
    let _ = glue_of;
    None
}
