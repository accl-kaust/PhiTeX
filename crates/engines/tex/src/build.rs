//! Building lists and boxes (§1029–§1135): the mode as a binding, the
//! current list as the contributions tagged with its list, characters
//! and ligatures (§1034–§1040), spaces, paragraphs and their line
//! breaking, boxes, insertions, discretionaries and accents.
//!
//! The mode and the current list are the run's semantic nest
//! ([`crate::nest`]): a paragraph, a box, a group opened in this run is a
//! level of it, and its list is a `Vec` folded as it grows. Level 0 is
//! the engine's scope the node runs in; a list there (the main vertical
//! list, a box a format line left open) is the contributions to the
//! accumulator `items` that carry its id, folded by the reader
//! ([`Run::list_fold`]).

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use phitex_defuse::{Effect, Value, ValueId, Values};
use phitex_typeset::glue::Glue;
use phitex_typeset::node::{BoxKind, BoxNode, Disc, GlueKind, KernKind, Ligature, List, Node};
use phitex_typeset::pack::{PackParams, Spec};

use crate::control::{LEADER_FLAG, SHIP_OUT_FLAG};
use crate::fields::Par;
use crate::nodes::{self, Side, T_BUILD_PAGE, T_DELETE, T_PD, T_SF, T_SPACE, T_VBOX};
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
    /// The mode and current list (§211): the innermost level of the nest,
    /// or, with none open, the scope kind the node runs in.
    pub(crate) fn mode(&mut self) -> R<Mode> {
        let base = self.base_mode()?;
        if self.nest.is_empty() {
            return Ok(base);
        }
        let groups = self.local_groups();
        // the vertical list a paragraph belongs to: the level below it
        let (vlid, vm) = self
            .nest
            .iter()
            .rev()
            .find(|l| l.mode.abs() == 1 && l.owns)
            .map_or((base.vlid, base.vm), |l| (l.lid, l.mode));
        // a paragraph level (group 0) is inside the group of the nearest
        // level below it that is a group, or of level 0
        let group = self
            .nest
            .iter()
            .rev()
            .find(|l| l.group != 0)
            .map_or(base.group, |l| l.group);
        let t = self.nest.last().expect("a level");
        Ok(Mode {
            m: t.mode,
            km: if t.group == 0 { vm } else { t.mode },
            lid: t.lid,
            vlid,
            group,
            sid: t.lid,
            depth: base.depth + groups,
            extra: t.extra.clone(),
            vm,
        })
    }

    /// Level 0: the scope kind the node runs in, decoded once per value.
    pub(crate) fn base_mode(&mut self) -> R<Mode> {
        let k = self.scope_kind()?;
        let v = self.v;
        let (group, sid, lid, km, depth, extra) = match k {
            Some(k) => match &self.kind_memo {
                Some((kk, p)) if *kk == k => p.clone(),
                _ => {
                    let xs = list_of(v, k);
                    let n = |i: usize| xs.get(i).map_or(0, |&x| int_of(v, x));
                    let p = (
                        n(0),
                        n(1),
                        n(2),
                        i32::try_from(n(3)).unwrap_or(1),
                        n(4),
                        xs.get(6..).map(<[ValueId]>::to_vec).unwrap_or_default(),
                    );
                    self.kind_memo = Some((k, p.clone()));
                    p
                }
            },
            None => (0, 0, 0, 1, 0, Vec::new()),
        };
        Ok(Mode {
            m: km,
            km,
            lid,
            vlid: lid,
            group,
            sid,
            depth,
            extra,
            vm: km,
        })
    }

    /// `{` and `\begingroup` (§1063): a level of the nest sharing the
    /// current list. The group is the run's: a node closes every group
    /// it opens (one still open at a format node's end is an error).
    #[allow(clippy::unnecessary_wraps)]
    pub(crate) fn open_group(&mut self, md: &Mode, grp: i64, extra: Vec<ValueId>) -> R<()> {
        self.push_group(md.m, md.lid, grp, extra);
        Ok(())
    }

    /// The end of a simple group (§1063): its locals restored, its
    /// `\aftergroup` tokens put back ([`Run::pop_level`]).
    pub(crate) fn close_simple(&mut self, md: &Mode) -> R<()> {
        self.close_group(md)?;
        Ok(())
    }

    /// Closes the innermost group: one of this run's (popped, nothing
    /// reaches the engine), or the engine scope the node runs in (a
    /// format line's `}` or `\endgroup`: `unsave` §281–§282 is the
    /// engine's, the `\aftergroup` tokens and the definitions that must
    /// reach out of the group are this node's). Returns the mode outside
    /// it.
    pub(crate) fn close_group(&mut self, md: &Mode) -> R<Mode> {
        if self.nest.last().is_some_and(|l| l.group != 0) {
            self.pop_level();
            return self.mode();
        }
        // a group of this run below a paragraph begun inside it
        // (`{\a text} more`): groups and the nest are apart in tex.web
        // (`unsave` leaves `nest` alone, §1063), so the group closes and
        // the paragraph goes on, the group's definitions undone
        if let Some(k) = self.nest.iter().rposition(|l| l.group != 0) {
            let above = self.nest.split_off(k + 1);
            self.pop_level();
            let (_, floor_s, floor_d, _) = self.st.open();
            for mut l in above {
                l.save_base = l.save_base.min(floor_s);
                l.def_base = l.def_base.min(floor_d);
                self.nest.push(l);
            }
            return self.mode();
        }
        self.effect = Effect::Close;
        self.unsave_extras(md)?;
        self.kind_outer(md)
    }

    /// The groups still open when the run ends. Only `\end` inside
    /// groups (§1054) leaves any: nothing after it reads them, so what
    /// they own goes to the main list (lid 0). A format node closes its
    /// own groups; one open at its end is an error.
    pub(crate) fn end_open(&mut self) {
        let nest = std::mem::take(&mut self.nest);
        self.abandon_levels(&nest);
        for l in nest {
            if self.claim_rest && l.owns {
                for x in l.list {
                    self.append(0, x);
                }
            }
        }
    }

    /// Leaves the run's one open group (sharing the list it is in) to
    /// the engine as a scope (`new_save_level`, §274): its kind is
    /// `[group, sid, list, mode, depth, parent kind, extra…]`, the
    /// following siblings' level 0 ([`Run::base_mode`]). Its local
    /// definitions are the run's, made inside the scope (`before`); its
    /// `\aftergroup` tokens the scope's `ag` list.
    pub(crate) fn leave_scope(&mut self) -> R<()> {
        let v = self.v;
        let base = self.base_mode()?;
        let parent = self.scope_kind()?.unwrap_or(ValueId::UNIT);
        let Some(l) = self.nest.pop() else {
            return Ok(());
        };
        self.abandon_levels(std::slice::from_ref(&l));
        let sid = i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes());
        let mut xs = vec![
            v.int(l.group),
            v.int(sid),
            v.int(base.lid),
            v.int(i64::from(l.mode)),
            v.int(base.depth + 1),
            parent,
        ];
        xs.extend(l.extra.iter().copied());
        let kind = v.list(xs);
        if !l.ag.is_empty() {
            let mut xs = vec![v.int(sid)];
            xs.extend(l.ag.iter().map(|t| t.encode(v)));
            self.set("ag", v.list(xs), false);
        }
        self.effect = Effect::Scope(phitex_defuse::Brackets {
            closes: 0,
            opens: 1,
            before: true,
            kind: Some(kind),
            kind_pending: false,
        });
        Ok(())
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
            let mut seen = phitex_defuse::FxHashSet::default();
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

    // ------------------------------------------------------------ lists

    /// `tail_append` to list `lid`: a list of this run's grows here, its
    /// transitions folded; a list of the engine's scope (the main
    /// vertical list, a box a format line left open) gets a contribution.
    pub(crate) fn append(&mut self, lid: i64, node: ValueId) {
        let v = self.v;
        if let Some(l) = self.level_of(lid) {
            fold_item(v, lid, node, &mut l.fold, &mut l.list);
            return;
        }
        let e = v.list(vec![v.int(lid), node]);
        let s = v.seq_unit(e);
        self.contribute("items", s);
    }

    /// The contents of list `lid` so far, and how many of them are
    /// still contributions (after the last `build_page`): the items its
    /// opener appended, then the contributions after the opener (the main
    /// list, opened by no node: from the unit's start).
    pub(crate) fn list_items(&mut self, lid: i64) -> R<(Vec<ValueId>, usize)> {
        if let Some(l) = self.level_of(lid) {
            return Ok((l.list.clone(), l.list.len()));
        }
        let f = self.list_fold(lid)?;
        // the page builder's markers are no list material
        let v = self.v;
        let items = f.items.into_iter().filter(|&x| !is_marker(v, x)).collect();
        Ok((items, f.since_bp))
    }

    /// The last item of list `lid` and the count [`Self::list_items`]
    /// gives, without copying a local list.
    pub(crate) fn list_last(&mut self, lid: i64) -> R<(Option<ValueId>, usize)> {
        if let Some(l) = self.level_of(lid) {
            return Ok((l.list.last().copied(), l.list.len()));
        }
        let (items, since) = self.list_items(lid)?;
        Ok((items.last().copied(), since))
    }

    /// [`Self::list_items`] with the space factor at the list's end: the
    /// list's transitions are folded left to right from 1000 (§1091,
    /// §1083), and each waiting space gets its glue from the factor where
    /// it stands (§1041–§1044).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn list_fold(&mut self, lid: i64) -> R<ListFold> {
        let v = self.v;
        let f = v.field("items");
        // a list is this run's or the engine's (lid 0): no list is opened
        // by one node and read by another
        let all = self.cx.acc_range(f, None)?;
        let mut out: Vec<ValueId> = Vec::new();
        let mut st = FoldState {
            sf: 1000,
            // §215, §1083, §1099, §1025: a vertical list begins with the
            // depth ignored
            pd: IGNORE_DEPTH,
            since_bp: 0,
        };
        let own: Vec<ValueId> = self
            .contribs
            .iter()
            .filter(|(g, _)| *g == f)
            .map(|(_, x)| *x)
            .collect();
        fold_entries(v, lid, &v.seq_to_vec(all), &mut st, &mut out);
        let mut push = |x: ValueId, out: &mut Vec<ValueId>| {
            let Value::List(xs) = &*v.get(x) else { return };
            if xs.len() != 2 || int_of(v, xs[0]) != lid {
                return;
            }
            fold_item(v, lid, xs[1], &mut st, out);
        };
        for s in own {
            for x in v.seq_to_vec(s) {
                push(x, &mut out);
            }
        }
        Ok(ListFold {
            items: out,
            since_bp: st.since_bp,
            sf: st.sf,
            pd: st.pd,
        })
    }

    /// `\\prevdepth` (§679): the fold of list `md.vlid`'s boxes and
    /// transitions so far.
    pub(crate) fn prev_depth(&mut self, md: &Mode) -> R<i32> {
        if let Some(l) = self.level_of(md.vlid) {
            return Ok(l.fold.pd);
        }
        Ok(self.list_fold(md.vlid)?.pd)
    }

    /// Sets the previous depth: a transition on the list, folded by its
    /// reader.
    pub(crate) fn set_prev_depth(&mut self, md: &Mode, d: i32) {
        let m = pd_set(self.v, d);
        self.append(md.vlid, m);
    }

    /// `\\spacefactor` (§1034): the fold of list `md.lid`'s transitions
    /// so far.
    pub(crate) fn space_factor(&mut self, md: &Mode) -> R<i32> {
        if let Some(l) = self.level_of(md.lid) {
            return Ok(l.fold.sf);
        }
        Ok(self.list_fold(md.lid)?.sf)
    }

    /// Sets the space factor: a transition on the list, not a definition
    /// (no later command reads it; the list's reader folds it).
    pub(crate) fn set_space_factor(&mut self, md: &Mode, s: i32) {
        let m = sf_set(self.v, s);
        self.append(md.lid, m);
    }

    /// `append_to_vlist` (§679): the box with the interline glue
    /// parameters in force here; the list's reader makes the glue from
    /// the depth before it ([`Self::list_fold`]).
    pub(crate) fn append_to_vlist(&mut self, md: &Mode, b: ValueId) -> R<()> {
        let v = self.v;
        let bs = self.gpar(Par::baselineskip)?;
        let ls = self.gpar(Par::lineskip)?;
        let lsl = self.ipar(Par::lineskiplimit)?;
        let x = v.list(vec![
            v.int(nodes::T_VBOX),
            b,
            crate::val::glue_value(v, &bs),
            crate::val::glue_value(v, &ls),
            v.int(i64::from(lsl)),
        ]);
        self.append(md.vlid, x);
        Ok(())
    }

    // -------------------------------------------------------- paragraphs

    /// `norm_min` (§1091).
    pub(crate) fn norm_min(h: i32) -> i64 {
        i64::from(h.clamp(1, 63))
    }

    /// `new_graf` (§1091).
    pub(crate) fn new_graf(&mut self, md: &Mode, indented: bool) -> R<()> {
        let v = self.v;
        self.set_prev_graf(md, 0);
        let nonempty = if md.km == 1 {
            true
        } else {
            self.list_last(md.vlid)?.0.is_some()
        };
        if nonempty {
            let g = self.gpar(Par::parskip)?;
            let n = nodes::glue_node(v, &g, GlueKind::Param(2));
            self.append(md.vlid, n);
        }
        let lang = i64::from(phitex_typeset::hyph::cur_lang(self.ipar(Par::language)?));
        let lhm = Self::norm_min(self.ipar(Par::lefthyphenmin)?);
        let rhm = Self::norm_min(self.ipar(Par::righthyphenmin)?);
        let packed = (lhm * 64 + rhm) * 65536 + lang;
        // the paragraph: a level of the nest (§1091 `push_nest`)
        let lid = self.push_list(102, 0, Vec::new());
        if let Some(l) = self.top_mut() {
            l.par = Some((lang, packed));
        }
        if indented {
            let w = self.ipar(Par::parindent)?;
            let b = null_box(v, w);
            self.append(lid, b);
        }
        let ep = self.par(Par::everypar)?;
        let toks = crate::val::toks_of(v, ep);
        self.push_toks(toks);
        self.build_page_par()
    }

    /// `\prevgraf` of the vertical list `md.vlid` (§1091, §1096): the
    /// level's, or the field `pg:<list>` of a list of the engine's scope.
    pub(crate) fn prev_graf(&mut self, md: &Mode) -> R<i32> {
        if let Some(l) = self.level_of(md.vlid) {
            return Ok(i32::try_from(l.prev_graf).unwrap_or(0));
        }
        self.int32(&format!("pg:{}", md.vlid))
    }

    pub(crate) fn set_prev_graf(&mut self, md: &Mode, n: i32) {
        if let Some(l) = self.level_of(md.vlid) {
            l.prev_graf = i64::from(n);
            return;
        }
        let v = self.v;
        self.set(&format!("pg:{}", md.vlid), v.int(i64::from(n)), true);
    }

    /// `normal_paragraph` (§1070).
    pub(crate) fn normal_paragraph(&mut self) -> R<()> {
        let v = self.v;
        if self.ipar(Par::looseness)? != 0 {
            self.set_par(Par::looseness, v.int(0), false);
        }
        if self.ipar(Par::hangindent)? != 0 {
            self.set_par(Par::hangindent, v.int(0), false);
        }
        if self.ipar(Par::hangafter)? != 1 {
            self.set_par(Par::hangafter, v.int(1), false);
        }
        if self
            .get_loc(crate::fields::cell::PARSHAPE)?
            .is_some_and(|x| !list_of(v, x).is_empty())
        {
            self.set_cell(crate::fields::cell::PARSHAPE, v.list(Vec::new()), false);
        }
        crate::etex::normal_paragraph(self)
    }

    /// `head_for_vmode` (§1095).
    pub(crate) fn head_for_vmode(&mut self, md: &Mode) {
        if md.m < 0 {
            // off_save (or "You can't use \hrule here")
            return;
        }
        self.back_cur();
        self.back_input(Tok::cs("par"));
    }

    /// `end_graf` (§1096).
    pub(crate) fn end_graf(&mut self, md: &Mode) -> R<()> {
        if md.m == 102 {
            // the paragraph's level is popped (§1096 `pop_nest`); its
            // lines go to the vertical list below it
            let items = match self.top() {
                Some(l) if l.group == 0 && l.mode == 102 => {
                    let l = self.pop_level().expect("the paragraph's level");
                    self.last_par = l.par.unwrap_or((0, 0));
                    l.list
                }
                _ => match self.par_level_under_groups() {
                    Some(k) => self.end_graf_under_groups(k)?,
                    None => return Ok(()),
                },
            };
            // §1096: error_count:=0
            self.error_count = 0;
            if !items.is_empty() {
                self.line_break(md, &items, false)?;
            }
            self.normal_paragraph()?;
        }
        Ok(())
    }

    /// The paragraph's level when groups opened inside the paragraph
    /// are still open above it (`abc{def\\par}`): the innermost level,
    /// under groups that share its list only.
    fn par_level_under_groups(&self) -> Option<usize> {
        let k = self
            .nest
            .iter()
            .rposition(|l| l.group == 0 && l.mode == 102 && l.owns)?;
        let lid = self.nest[k].lid;
        self.nest[k + 1..]
            .iter()
            .all(|l| l.group != 0 && !l.owns && l.lid == lid)
            .then_some(k)
    }

    /// Ends the paragraph at level `k` under the groups opened in it:
    /// the semantic nest and the groups are apart in TeX (§1096
    /// `pop_nest` pops the paragraph whatever groups are open), so the
    /// groups stay open, now in the vertical list the paragraph was in
    /// (their saved definitions restored at their own close, as ever).
    /// Returns the paragraph's list.
    fn end_graf_under_groups(&mut self, k: usize) -> R<Vec<ValueId>> {
        let (mode, lid) = if let Some(b) = k.checked_sub(1) {
            (self.nest[b].mode, self.nest[b].lid)
        } else {
            let base = self.base_mode()?;
            (base.m, base.lid)
        };
        let l = self.nest.remove(k);
        for g in &mut self.nest[k..] {
            g.mode = mode;
            g.lid = lid;
        }
        self.last_par = l.par.unwrap_or((0, 0));
        Ok(l.list)
    }

    /// The fonts the nodes use.
    pub(crate) fn font_view(&mut self, list: &[Node]) -> R<crate::fonts::View> {
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
                    Node::Unset(u) => walk(&u.list, ids),
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
        let mut ids = std::collections::BTreeSet::new();
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
                badness: self.ipar(Par::hbadness)?,
                fuzz: self.ipar(Par::hfuzz)?,
                overfull_rule: self.ipar(Par::overfullrule)?,
            }
        } else {
            PackParams {
                badness: self.ipar(Par::vbadness)?,
                fuzz: self.ipar(Par::vfuzz)?,
                overfull_rule: 0,
            }
        })
    }

    /// `line_break` (§815) and `post_line_break` (§877–§890): the lines
    /// go to the enclosing vertical list.
    #[allow(clippy::too_many_lines)]
    /// `line_break` (§815): the lines go to the vertical list; the last
    /// line's box is returned (`just_box`, for a display, §1146).
    pub(crate) fn line_break(
        &mut self,
        md: &Mode,
        items: &[ValueId],
        display: bool,
    ) -> R<Option<BoxNode>> {
        let v = self.v;
        let mut side = Side::default();
        let list: List = items
            .iter()
            .filter_map(|&x| nodes::decode(v, x, &mut side))
            .collect();
        let view = self.font_view(&list)?;
        let shape = self
            .get_loc(crate::fields::cell::PARSHAPE)?
            .map(|x| list_of(v, x))
            .unwrap_or_default();
        let par_shape: Vec<(i32, i32)> = shape
            .chunks(2)
            .map(|c| {
                (
                    i32::try_from(int_of(v, c[0])).unwrap_or(0),
                    c.get(1)
                        .map_or(0, |&w| i32::try_from(int_of(v, w)).unwrap_or(0)),
                )
            })
            .collect();
        let pg_packed = self.par_packed(md);
        let lang = phitex_typeset::linebreak::LangState {
            lang: u16::try_from(pg_packed % 65536).unwrap_or(0),
            lhm: u8::try_from((pg_packed / 65536) / 64).unwrap_or(2),
            rhm: u8::try_from((pg_packed / 65536) % 64).unwrap_or(3),
        };
        let params = phitex_typeset::linebreak::LineBreakParams {
            pretolerance: self.ipar(Par::pretolerance)?,
            tolerance: self.ipar(Par::tolerance)?,
            emergency_stretch: self.ipar(Par::emergencystretch)?,
            looseness: self.ipar(Par::looseness)?,
            line_penalty: self.ipar(Par::linepenalty)?,
            hyphen_penalty: self.ipar(Par::hyphenpenalty)?,
            ex_hyphen_penalty: self.ipar(Par::exhyphenpenalty)?,
            adj_demerits: self.ipar(Par::adjdemerits)?,
            double_hyphen_demerits: self.ipar(Par::doublehyphendemerits)?,
            final_hyphen_demerits: self.ipar(Par::finalhyphendemerits)?,
            left_skip: self.gpar(Par::leftskip)?,
            right_skip: self.gpar(Par::rightskip)?,
            par_fill_skip: self.gpar(Par::parfillskip)?,
            hsize: self.ipar(Par::hsize)?,
            hang_indent: self.ipar(Par::hangindent)?,
            hang_after: self.ipar(Par::hangafter)?,
            par_shape,
            last_line_fit: self.ipar(Par::lastlinefit)?,
            inter_line_penalty: self.ipar(Par::interlinepenalty)?,
            club_penalty: self.ipar(Par::clubpenalty)?,
            widow_penalty: self.ipar(Par::widowpenalty)?,
            display_widow_penalty: self.ipar(Par::displaywidowpenalty)?,
            broken_penalty: self.ipar(Par::brokenpenalty)?,
            inter_line_penalties: crate::etex::penalties(self, 1)?,
            club_penalties: crate::etex::penalties(self, 2)?,
            widow_penalties: crate::etex::penalties(self, 3)?,
            display_widow_penalties: crate::etex::penalties(self, 4)?,
            prev_graf: self.prev_graf(md)?,
            lang,
            pack: self.pack_params(true)?,
        };
        let patterns = self.patterns()?;
        // \lccode of the characters the hyphenator can meet: those of the
        // list (a character not in it is never asked for)
        let mut lc = [0u16; 256];
        let mut seen = [false; 256];
        chars_of(&list, &mut seen);
        for (c, x) in lc.iter_mut().enumerate() {
            if seen[c] {
                *x = u16::try_from(i64::from(self.code(0, c)?)).unwrap_or(0);
            }
        }
        let uc_hyph = self.ipar(Par::uchyph)?;
        let hyphen = view.hyphen.clone();
        let lcf = |c: u16| lc.get(usize::from(c)).copied().unwrap_or(0);
        let hcf = |f: u32| hyphen.get(&f).copied().unwrap_or(-1);
        let mut hy = phitex_typeset::hyph::TexHyphenator {
            patterns: &patterns,
            lc_code: &lcf,
            uc_hyph,
            hyphen_char: &hcf,
        };
        let para = phitex_typeset::linebreak::line_break(list, display, &params, &view, &mut hy);
        let mut vm = md.clone();
        vm.lid = md.vlid;
        let just_box = para.lines.last().map(|l| l.packed.node.clone());
        for line in para.lines {
            self.box_report(true, line.packed.report, true);
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
        self.set_prev_graf(md, para.prev_graf);
        Ok(just_box)
    }

    /// The paragraph's packed language and hyphen minima (§1091), from
    /// the level that was its (just popped by `end_graf`); 0 outside a
    /// paragraph.
    fn par_packed(&self, md: &Mode) -> i64 {
        let _ = md;
        self.last_par.1
    }

    /// The hyphenation patterns and exceptions so far, entered once per
    /// value for the process.
    fn patterns(&mut self) -> R<Arc<phitex_typeset::hyph::Patterns>> {
        type Cache = Mutex<phitex_defuse::FxHashMap<u64, Arc<phitex_typeset::hyph::Patterns>>>;
        static C: OnceLock<Cache> = OnceLock::new();
        let x = self.get_loc(crate::fields::cell::PATTERNS)?;
        let key = x.map_or(0, ValueId::raw);
        let cache = C.get_or_init(|| Mutex::new(phitex_defuse::FxHashMap::default()));
        if let Some(p) = cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
        {
            return Ok(p.clone());
        }
        let v = self.v;
        let mut p = phitex_typeset::hyph::Patterns::default();
        if let Some(x) = x {
            for e in list_of(v, x) {
                let xs = list_of(v, e);
                let lang = u16::try_from(xs.first().map_or(0, |&l| int_of(v, l))).unwrap_or(0);
                let kind = xs
                    .get(1)
                    .map(|&k| crate::val::str_of(v, k))
                    .unwrap_or_default();
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

    /// The field of box register `n`, `box:<n>`, interned once per values
    /// epoch (a register access builds no name).
    pub(crate) fn box_field(&self, n: i64) -> phitex_defuse::Field {
        thread_local! {
            static BOXES: std::cell::RefCell<(u64, u64, Vec<Option<phitex_defuse::Field>>)> =
                const { std::cell::RefCell::new((u64::MAX, u64::MAX, Vec::new())) };
        }
        let v = self.v;
        let epoch = self.cx.values_epoch();
        let slot = usize::try_from(n).ok().filter(|&i| i < 32768);
        let make = || self.ops.fields.get_at(v, epoch, &format!("box:{n}"));
        let Some(i) = slot else { return make() };
        BOXES.with(|c| {
            let mut c = c.borrow_mut();
            if c.0 != v.instance() || c.1 != epoch {
                *c = (v.instance(), epoch, Vec::new());
            }
            if let Some(Some(f)) = c.2.get(i) {
                return *f;
            }
            let f = make();
            if c.2.len() <= i {
                c.2.resize(i + 1, None);
            }
            c.2[i] = Some(f);
            f
        })
    }

    /// The box in register `n` (`None`: void).
    pub(crate) fn box_reg(&mut self, n: i32) -> R<Option<ValueId>> {
        let x = {
            let f = self.box_field(i64::from(n));
            self.get_f(f)?
        };
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
        let f = self.box_field(n);
        self.set_f(f, val, global);
    }

    /// A box register changed "at the same level" (§1079 `box(n):=null`,
    /// §1247): the binding keeps its level and is replaced where it was
    /// made, so no group saves it and the groups opened above that level
    /// keep the change at their close ([`Run::modify_at`]).
    pub(crate) fn modify_box_reg(&mut self, n: i32, b: Option<ValueId>) -> R<()> {
        let v = self.v;
        let f = self.box_field(i64::from(n));
        let x = self.get_f(f)?;
        let lvl = x.map_or(0, |x| crate::val::int_at(v, x, 0));
        let val = v.list(vec![v.int(lvl), b.unwrap_or(ValueId::UNIT)]);
        let base = self.base_mode()?.depth;
        let l = self.loc_of(f);
        self.modify_at(l, val, lvl, base);
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
                    let g = if ctx < BOX_FLAG && md.m.abs() == 1 {
                        group::ADJUSTED_HBOX
                    } else {
                        group::HBOX
                    };
                    (g, self.scan_spec()?)
                } else if k == 1 {
                    (group::VBOX, self.scan_spec()?)
                } else {
                    (group::VTOP, self.scan_spec()?)
                };
                self.scan_left_brace()?;
                let extra = vec![v.int(ctx), v.int(spec.0), v.int(i64::from(spec.1))];
                let m = if k == 102 { -102 } else { -1 };
                // the box: a level of the nest (§1083 `push_nest`)
                self.push_list(m, grp, extra);
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
                    self.build_page_outer()?;
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
            let (n, global) = if ctx < BOX_FLAG + 256 {
                (ctx - BOX_FLAG, false)
            } else {
                (ctx - BOX_FLAG - 256, true)
            };
            self.set_box_reg(md, n, b, global);
        } else if let Some(b) = b {
            if ctx > SHIP_OUT_FLAG {
                // §1078
                self.skip_blanks_relax_x()?;
                if (self.cur.cmd == cmd::HSKIP && md.m.abs() != 1)
                    || (self.cur.cmd == cmd::VSKIP && md.m.abs() == 1)
                {
                    let g = self.glue_of_cur()?;
                    let kind = match ctx - LEADER_FLAG {
                        0 => GlueKind::ALeaders,
                        1 => GlueKind::CLeaders,
                        _ => GlueKind::XLeaders,
                    };
                    let mut side = Side::default();
                    let leader = nodes::decode(v, b, &mut side).map(Box::new);
                    let node = Node::Glue(phitex_typeset::node::GlueNode {
                        spec: g,
                        kind,
                        leader,
                    });
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
        let kind = if s > 4 {
            GlueKind::Mu
        } else {
            GlueKind::Normal
        };
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
        let (last, since) = self.list_last(md.lid)?;
        if md.m == 1 && since == 0 {
            // §1106: nothing can be taken from the current page
            return Ok(());
        }
        if last.is_some_and(|x| nodes::tag(self.v, x) == want) {
            let m = nodes::marker(self.v, T_DELETE);
            self.append(md.lid, m);
        }
        Ok(())
    }

    /// `unpackage` (§1110).
    pub(crate) fn unpackage(&mut self, md: &Mode) -> R<()> {
        let c = self.cur.chr;
        let n = self.scan_eight_bit_int()?;
        let Some(b) = self.box_reg(n)? else {
            return Ok(());
        };
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
        let Some(last) = self.list_last(md.lid)?.0 else {
            return Ok(());
        };
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
            let f = self.cint(crate::fields::cell::FONT)?;
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
            let extra = vec![
                v.int(0),
                v.int(md.lid),
                v.list(Vec::new()),
                v.list(Vec::new()),
            ];
            self.push_list(-102, group::DISC, extra);
        }
        Ok(())
    }

    /// `make_accent` (§1123–§1125).
    pub(crate) fn make_accent(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let a_ch = self.scan_char_num()?;
        let f = u32::try_from(self.cint(crate::fields::cell::FONT)?).unwrap_or(0);
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
        let a = view
            .char_dims(f, u32::try_from(a_ch).unwrap_or(0))
            .map_or(0, |d| d.width);
        self.do_assignments()?;
        // §1124
        let f2 = u32::try_from(self.cint(crate::fields::cell::FONT)?).unwrap_or(0);
        let mut view2 = crate::fonts::View::default();
        self.add_font(&mut view2, f2)?;
        let q: Option<u32> = match self.cur.cmd {
            cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN => {
                Some(u32::try_from(self.cur.chr).unwrap_or(0))
            }
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
                    let pp = phitex_typeset::pack::hpack(
                        vec![p],
                        Spec::Additional(0),
                        &view,
                        &PackParams::default(),
                        false,
                    );
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
        self.push_list(-1, group::INSERT, vec![v.int(i64::from(n)), v.int(md.lid)]);
        self.normal_paragraph()?;
        Ok(())
    }

    /// `handle_right_brace` (§1068, §1085, §1100, §1118).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn handle_right_brace(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        // a paragraph inside a vertical group ends first (`end_graf`)
        if md.m == 102
            && matches!(
                md.group,
                group::VBOX
                    | group::VTOP
                    | group::INSERT
                    | group::OUTPUT
                    | group::VCENTER
                    | group::NO_ALIGN
            )
        {
            let t = self.cur.tok;
            self.back_input(t);
            self.back_input(Tok::cs("\u{10FFFE}endgraf"));
            return Ok(());
        }
        match md.group {
            group::SIMPLE => self.close_simple(md),
            group::HBOX | group::ADJUSTED_HBOX | group::VBOX | group::VTOP => {
                // §1086 `package`
                let x = |i: usize| md.extra.get(i).map_or(0, |&y| int_of(v, y));
                let (ctx, spec_code, amount) = (x(0), x(1), i32::try_from(x(2)).unwrap_or(0));
                let spec = if spec_code == 0 {
                    Spec::Exactly(amount)
                } else {
                    Spec::Additional(amount)
                };
                let depth_limit = self.ipar(Par::boxmaxdepth)?;
                let (items, _) = self.list_items(md.lid)?;
                let mut side = Side::default();
                let mut kept = Vec::with_capacity(items.len());
                let list: List = items
                    .iter()
                    .filter_map(|&y| {
                        let n = nodes::decode(v, y, &mut side)?;
                        kept.push(y);
                        Some(n)
                    })
                    .collect();
                let adjusted = md.group == group::ADJUSTED_HBOX;
                let packed = if md.group == group::HBOX || adjusted {
                    let view = self.font_view(&list)?;
                    let pp = self.pack_params(true)?;
                    phitex_typeset::pack::hpack(list, spec, &view, &pp, adjusted)
                } else {
                    let pp = self.pack_params(false)?;
                    let mut p = phitex_typeset::pack::vpackage(list, spec, depth_limit, &pp);
                    if md.group == group::VTOP {
                        // §1087
                        let h = match p.node.list.first() {
                            Some(n @ (Node::Box(_) | Node::Rule(_))) => {
                                n.box_dims().map_or(0, |b| b.1)
                            }
                            _ => 0,
                        };
                        p.node.depth = p.node.depth - h + p.node.height;
                        p.node.height = h;
                    }
                    p
                };
                self.set_cell(
                    crate::fields::cell::BADNESS,
                    v.int(i64::from(packed.badness)),
                    true,
                );
                let h = md.group == group::HBOX || adjusted;
                self.box_report(h, packed.report, false);
                // packaging without adjustments keeps the list as it was:
                // its value is the items' (encoded once, when appended)
                let b = if !adjusted && packed.node.list.len() == kept.len() {
                    nodes::encode_box_with(v, &packed.node, v.list(kept))
                } else {
                    nodes::encode_box(v, &packed.node, &side)
                };
                let outer = self.close_group(md)?;
                if (BOX_FLAG..SHIP_OUT_FLAG).contains(&ctx)
                    && ctx < BOX_FLAG + 256
                    && self.effect == Effect::Close
                {
                    // a local \setbox of an engine scope lands outside it
                    let lvl = v.int(outer.depth);
                    let val = v.list(vec![lvl, b]);
                    let def = v.list(vec![
                        v.str(&format!("box:{}", ctx - BOX_FLAG)),
                        val,
                        v.int(0),
                    ]);
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
                let n = md.extra.first().map_or(0, |&y| int_of(v, y));
                let q = self.gpar(Par::splittopskip)?;
                let d = self.ipar(Par::splitmaxdepth)?;
                let f = self.ipar(Par::floatingpenalty)?;
                let (items, _) = self.list_items(md.lid)?;
                let mut side = Side::default();
                let list: List = items
                    .iter()
                    .filter_map(|&y| nodes::decode(v, y, &mut side))
                    .collect();
                let p =
                    phitex_typeset::pack::vpack(list, Spec::Additional(0), &PackParams::default());
                let outer = self.close_group(md)?;
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
                self.build_page_outer()
            }
            group::DISC => self.build_discretionary(md),
            group::OUTPUT => crate::page::output_end(self, md),
            group::MATH => self.math_group_end(md),
            group::ALIGN => self.align_right_brace(md),
            group::NO_ALIGN => self.noalign_end(md),
            group::VCENTER => self.vcenter_end(md),
            group::MATH_CHOICE => self.build_choices(md),
            // MATH_SHIFT, MATH_LEFT: "Extra }, or forgotten $" / "\right"
            // (§1069); SEMI_SIMPLE: off_save
            _ => Ok(()),
        }
    }

    /// The mode outside the innermost group (what `pop_nest` returns to)
    /// without closing it: the level below, or the engine scope's parent.
    pub(crate) fn outer_mode(&mut self, md: &Mode) -> R<Mode> {
        if let Some(l) = self.nest.pop_if(|l| l.group != 0) {
            let m = self.mode();
            self.nest.push(l);
            return m;
        }
        self.kind_outer(md)
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
            vm: km,
        })
    }

    /// `build_discretionary` (§1119–§1121).
    fn build_discretionary(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let part = md.extra.first().map_or(0, |&y| int_of(v, y));
        let (items, _) = self.list_items(md.lid)?;
        let mut side = Side::default();
        let mut list: List = Vec::new();
        for &y in &items {
            if let Some(n) = nodes::decode(v, y, &mut side) {
                match n {
                    Node::Char { .. }
                    | Node::Kern(..)
                    | Node::Box(_)
                    | Node::Rule(_)
                    | Node::Lig(_) => list.push(n),
                    _ => break, // "Improper discretionary list"
                }
            }
        }
        let enc = nodes::encode_list(v, &list, &side);
        if part < 2 {
            let mut extra = md.extra.clone();
            extra[0] = v.int(part + 1);
            if part == 0 {
                extra[2] = enc;
            } else {
                extra[3] = enc;
            }
            // §1119: unsave, then the next part's group
            self.pop_level();
            self.scan_left_brace()?;
            self.push_list(-102, group::DISC, extra);
            return Ok(());
        }
        let outer = self.close_group(md)?;
        let mut s2 = Side::default();
        let pre = md
            .extra
            .get(2)
            .map_or_else(Vec::new, |&l| nodes::decode_list(v, l, &mut s2));
        let post = md
            .extra
            .get(3)
            .map_or_else(Vec::new, |&l| nodes::decode_list(v, l, &mut s2));
        let d = Node::Disc(Disc {
            pre,
            post,
            replace: list,
        });
        // the replacement list follows the disc node in tex.web's list;
        // here it is inside it
        let y = nodes::encode(v, &d, &merge_sides(s2, side));
        self.append(outer.lid, y);
        Ok(())
    }

    // ------------------------------------------------------- characters

    /// Characters and spaces (§1030–§1044): `main_loop` and the spaces
    /// after it, while the input lasts without claiming more.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn char_run(&mut self, md: &Mode, cancel: bool) -> R<()> {
        let v = self.v;
        let f = u32::try_from(self.cint(crate::fields::cell::FONT)?).unwrap_or(0);
        let mut view = crate::fonts::View::default();
        self.add_font(&mut view, f)?;
        // the space factor is never read: the run's effect on it so far
        // (`sf`: SF_UNREAD none, n >= 0 set to n, -s §1034's update by s)
        // goes on the list as a transition before each space that needs
        // the factor, and at the end; the list's reader folds them
        let mut sf = SF_UNREAD;
        let mut out: Vec<Node> = Vec::new();
        // transitions and waiting spaces, each before `out[k]`
        let mut raws: Vec<(usize, ValueId)> = Vec::new();
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
                    let next = self.main_loop(
                        md,
                        f,
                        &view,
                        u32::try_from(ch).unwrap_or(0),
                        &mut sf,
                        &mut out,
                        &mut cancel_boundary,
                    )?;
                    if next {
                        continue;
                    }
                }
                cmd::NO_BOUNDARY => {
                    self.get_x_token()?;
                    if matches!(
                        self.cur.cmd,
                        cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN | cmd::CHAR_NUM
                    ) {
                        cancel_boundary = true;
                    }
                    continue;
                }
                cmd::SPACER | cmd::EX_SPACE => {
                    if c == cmd::EX_SPACE || sf >= 0 {
                        // the factor is known here (or not needed)
                        let normal = c == cmd::EX_SPACE || sf == 1000;
                        let g = self.space_glue(f, sf, normal)?;
                        out.push(g);
                    } else {
                        if sf != SF_UNREAD {
                            raws.push((out.len(), sf_value(v, sf)));
                            sf = SF_UNREAD;
                        }
                        let p = self.waiting_space(f)?;
                        raws.push((out.len(), p));
                    }
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
                cmd::LETTER
                    | cmd::OTHER_CHAR
                    | cmd::CHAR_GIVEN
                    | cmd::CHAR_NUM
                    | cmd::NO_BOUNDARY
                    | cmd::SPACER
                    | cmd::EX_SPACE
            ) {
                self.back_cur();
                break;
            }
        }
        if sf != SF_UNREAD {
            raws.push((out.len(), sf_value(v, sf)));
        }
        let side = Side::default();
        let mut raws = raws.into_iter().peekable();
        for (k, n) in out.iter().enumerate() {
            while let Some((_, r)) = raws.next_if(|&(at, _)| at == k) {
                self.append(md.lid, r);
            }
            let x = nodes::encode(v, n, &side);
            self.append(md.lid, x);
        }
        for (_, r) in raws {
            self.append(md.lid, r);
        }
        Ok(())
    }

    /// A space whose glue waits for the space factor: the parameters in
    /// force here, for the list's reader ([`resolve_space`]).
    fn waiting_space(&mut self, f: u32) -> R<ValueId> {
        let v = self.v;
        let ss = self.gpar(Par::spaceskip)?;
        let xs = self.gpar(Par::xspaceskip)?;
        let fi = i64::from(f);
        let mut xs_v = vec![
            v.int(nodes::T_SPACE),
            crate::val::glue_value(v, &ss),
            crate::val::glue_value(v, &xs),
        ];
        for k in [2, 3, 4, 7] {
            xs_v.push(v.int(i64::from(self.font_dimen(fi, k)?)));
        }
        Ok(v.list(xs_v))
    }

    /// A space (§1041–§1044).
    pub(crate) fn space_glue(&mut self, f: u32, sf: i32, normal: bool) -> R<Node> {
        let ss = self.gpar(Par::spaceskip)?;
        let fi = i64::from(f);
        let zero = |g: &Glue| g.width == 0 && g.stretch == 0 && g.shrink == 0;
        if normal && !zero(&ss) {
            return Ok(Node::param_glue(12, ss));
        }
        let xs = if normal {
            Glue::ZERO
        } else {
            self.gpar(Par::xspaceskip)?
        };
        let need_font = zero(&ss) || (normal && zero(&ss));
        let d = if need_font {
            [
                self.font_dimen(fi, 2)?,
                self.font_dimen(fi, 3)?,
                self.font_dimen(fi, 4)?,
                if sf >= 2000 {
                    self.font_dimen(fi, 7)?
                } else {
                    0
                },
            ]
        } else {
            [
                0,
                0,
                0,
                if sf >= 2000 {
                    self.font_dimen(fi, 7)?
                } else {
                    0
                },
            ]
        };
        Ok(space_node(&ss, &xs, d, sf, normal))
    }

    /// `adjust_space_factor` (§1034) on the run's effect `sf` (see
    /// [`Self::char_run`]): an `sf_code` above 1000 needs the old factor
    /// only through "below 1000?", so with the old factor unknown it is
    /// kept as the update itself (`-s`), never read.
    fn adjust_sf(&mut self, _md: &Mode, ch: u32, sf: &mut i32) -> R<()> {
        let s = self.code(2, ch)?;
        if s == 1000 {
            *sf = 1000;
        } else if s < 1000 {
            if s > 0 {
                *sf = s;
            }
        } else if *sf == SF_UNREAD {
            *sf = -s;
        } else if *sf >= 0 && *sf < 1000 {
            *sf = 1000;
        } else {
            // a known factor of 1000 or more, or an update (whose result
            // is 1000 or more): `s`
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
        self.adjust_sf(md, ch, sf)?;
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
                    at = if lig_stack.last().is_some_and(|t| t.lig) {
                        L::MoveLig
                    } else {
                        L::Move2
                    };
                }
                L::Move2 => {
                    // §1036 `main_loop_move+2`: the character appended is
                    // the input's (the lig stack's top, `cur_chr`), which
                    // after a `=:` ligature (§1040) is not `cur_l`
                    let cl = cur_l.unwrap_or(0);
                    let input = lig_stack.last().map_or(cl, |t| t.ch);
                    if !view.exists(f, cl) {
                        // char_warning (§581)
                        lig_stack.pop();
                        self.char_warning(f, input)?;
                        return Ok(false);
                    }
                    out.push(Node::Char {
                        font: f,
                        ch: u16::try_from(input).unwrap_or(0),
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
                        if !matches!(
                            self.cur.cmd,
                            cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN
                        ) {
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
                    if let Some(c) = got {
                        self.adjust_sf(md, c, sf)?;
                        lig_stack.push(Item {
                            ch: c,
                            lig: false,
                            ptr: None,
                        });
                        cur_r = Some(c);
                        if cur_r == false_bchar {
                            cur_r = None;
                        }
                    } else {
                        cur_r = bchar;
                        lig_stack.clear();
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
                            at = if lig_stack.is_empty() {
                                L::Wrapup
                            } else {
                                L::Move1
                            };
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

    /// The box reports of §660–§667 and §674–§678, as diagnostics (the
    /// log keeps the oracle's text).
    fn box_report(&mut self, h: bool, r: Option<phitex_typeset::pack::Report>, in_par: bool) {
        use phitex_typeset::pack::Report;
        let Some(r) = r else { return };
        let what = if h { "\\hbox" } else { "\\vbox" };
        let mut s = match r {
            Report::Underfull(b) => format!("Underfull {what} (badness {b})"),
            Report::Loose(b) => format!("Loose {what} (badness {b})"),
            Report::Tight(b) => format!("Tight {what} (badness {b})"),
            Report::Overfull(x) => {
                let mut d = String::new();
                crate::print::print_scaled(&mut d, x);
                format!(
                    "Overfull {what} ({d}pt too {})",
                    if h { "wide" } else { "high" }
                )
            }
        };
        if in_par {
            s.push_str(" in paragraph");
        }
        self.diag(false, &s);
    }

    /// `char_warning` (§581).
    fn char_warning(&mut self, f: u32, c: u32) -> R<()> {
        if self.ipar(Par::tracinglostchars)? > 0 {
            let mut s = String::from("Missing character: There is no ");
            crate::print::push_code(&mut s, c);
            s.push_str(" in font ");
            let info = self.get(&format!("fontinfo:{f}"))?;
            if let Some(i) = crate::fonts::info(self.v, info) {
                s.push_str(&i.name);
            }
            s.push('!');
            self.diag(false, &s);
            self.log("diag", &s)?;
        }
        Ok(())
    }

    /// `\lastpenalty`, `\lastkern`, `\lastskip`, `\inputlineno`,
    /// `\badness` (§424).
    pub(crate) fn last_item(&mut self, m: i32) -> R<crate::scan::IVal> {
        use crate::scan::IVal;
        if m >= 100 {
            return crate::etex::last_item(self, m);
        }
        if m > 2 {
            return Ok(IVal::Int(if m == 3 {
                self.input_line_no()?
            } else {
                self.int_loc(crate::fields::cell::BADNESS)?
            }));
        }
        let md = self.mode()?;
        let (items, _) = self.list_items(md.lid)?;
        let v = self.v;
        let last = items.last().copied();
        let tag = last.map_or(-1, |x| nodes::tag(v, x));
        Ok(match m {
            0 => IVal::Int(if tag == nodes::T_PENALTY {
                nodes::int_field(v, last.unwrap_or(ValueId::UNIT), 1)
            } else {
                0
            }),
            1 => IVal::Dimen(if tag == nodes::T_KERN {
                nodes::int_field(v, last.unwrap_or(ValueId::UNIT), 1)
            } else {
                0
            }),
            _ => {
                if tag == nodes::T_GLUE {
                    let x = last.unwrap_or(ValueId::UNIT);
                    let g = nodes::glue_of_node(v, x);
                    if nodes::int_field(v, x, 2) == 2 {
                        IVal::Mu(g)
                    } else {
                        IVal::Glue(g)
                    }
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
        // the page builder (`page.rs`) empties the page: §1054's loop is
        // its, with the material `\end` appends each time round
        let v = self.v;
        let w = self.ipar(Par::hsize)?;
        let b = null_box(v, w);
        let fill = Glue {
            stretch: 65536,
            stretch_order: phitex_typeset::glue::Order::Fill,
            ..Glue::ZERO
        };
        let g = nodes::glue_node(v, &fill, GlueKind::Normal);
        let p = nodes::penalty_node(v, -0o10000000000);
        if self.pg.is_some() {
            // a text paging inline: §1054 as tex.web has it
            if self.all_over_inline()? {
                self.claim_rest = true;
                self.frames.clear();
                return Ok(());
            }
            self.back_cur();
            for x in [b, g, p] {
                self.append(0, x);
            }
            return self.build_page();
        }
        let m = v.list(vec![v.int(nodes::T_END), b, g, p]);
        self.append(0, m);
        // nothing after `\end` is read
        self.claim_rest = true;
        self.frames.clear();
        Ok(())
    }

    /// `\vsplit` (§977): the box split off register `n` at height `h`.
    fn vsplit(&mut self, n: i32, h: i32) -> R<Option<ValueId>> {
        let Some(b) = self.box_reg(n)? else {
            return Ok(None);
        };
        let v = self.v;
        let mut side = Side::default();
        let Some(Node::Box(bx)) = nodes::decode(v, b, &mut side) else {
            return Ok(None);
        };
        if bx.kind != BoxKind::V {
            return Ok(None);
        }
        let stk = self.gpar(Par::splittopskip)?;
        let smd = self.ipar(Par::splitmaxdepth)?;
        let pp = self.pack_params(false)?;
        let r = phitex_typeset::page::vsplit(bx.list, h, smd, stk, false, &pp);
        crate::etex::split_marks(self, &r.marks, &side)?;
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

/// Marks in `seen` the character codes below 256 of `list`, its
/// ligatures' originals and its nested lists included.
fn chars_of(list: &[Node], seen: &mut [bool; 256]) {
    fn mark(seen: &mut [bool; 256], c: u16) {
        if let Some(s) = seen.get_mut(usize::from(c)) {
            *s = true;
        }
    }
    for n in list {
        match n {
            Node::Char { ch, .. } => mark(seen, *ch),
            Node::Lig(l) => {
                mark(seen, l.ch);
                for &c in &l.original {
                    mark(seen, c);
                }
            }
            Node::Disc(d) => {
                chars_of(&d.pre, seen);
                chars_of(&d.post, seen);
                chars_of(&d.replace, seen);
            }
            Node::Box(b) => chars_of(&b.list, seen),
            Node::Ins(i) => chars_of(&i.list, seen),
            Node::Adjust(l) => chars_of(l, seen),
            Node::Unset(u) => chars_of(&u.list, seen),
            Node::Glue(g) => {
                if let Some(l) = &g.leader {
                    chars_of(std::slice::from_ref(&**l), seen);
                }
            }
            _ => {}
        }
    }
}

/// The space factor not read yet in a run of characters.
const SF_UNREAD: i32 = i32::MIN;

/// The state of a list's fold: the space factor (§1034), the previous
/// depth (§679), and the items since the last `build_page` marker.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FoldState {
    pub sf: i32,
    pub pd: i32,
    pub since_bp: usize,
}

/// One item appended to list `lid` folded into `out` (§1034's space
/// factor transitions, §679's interline glue, deletions): the list as its
/// reader sees it.
pub(crate) fn fold_item(
    v: &Values,
    lid: i64,
    n: ValueId,
    st: &mut FoldState,
    out: &mut Vec<ValueId>,
) {
    let _ = lid;
    match nodes::tag(v, n) {
        T_DELETE => {
            if st.since_bp > 0 {
                // the last node, seen through the page builder's markers
                if let Some(i) = out.iter().rposition(|&x| !is_marker(v, x)) {
                    out.remove(i);
                }
                st.since_bp -= 1;
            }
        }
        // the page builder's markers (page parameters, a marks read): kept
        // in place for its fold, invisible to the list's readers
        nodes::T_PAGE_PARAMS | nodes::T_MARKS_AT | nodes::T_WRITE_AT | nodes::T_FLUSH_AT => {
            out.push(n);
        }
        T_BUILD_PAGE => {
            st.since_bp = 0;
        }
        T_PD => st.pd = i32::try_from(crate::val::int_at(v, n, 1)).unwrap_or(0),
        T_VBOX => {
            let ys = list_of(v, n);
            let b = ys.get(1).copied().unwrap_or(ValueId::UNIT);
            let bs = crate::val::glue_of(v, ys.get(2).copied());
            let ls = crate::val::glue_of(v, ys.get(3).copied());
            let lsl = i32::try_from(crate::val::int_at(v, n, 4)).unwrap_or(0);
            let ht = nodes::box_dim(v, b, 3);
            if let Some(g) = phitex_typeset::linebreak::interline_glue(st.pd, ht, bs, ls, lsl) {
                out.push(nodes::encode(v, &g, &Side::default()));
                st.since_bp += 1;
            }
            out.push(b);
            st.since_bp += 1;
            st.pd = nodes::box_dim(v, b, 2);
        }
        T_SF => {
            let k = crate::val::int_at(v, n, 1);
            let s = i32::try_from(crate::val::int_at(v, n, 2)).unwrap_or(1000);
            st.sf = if k == 0 || st.sf >= 1000 { s } else { 1000 };
        }
        T_SPACE => {
            out.push(resolve_space(v, n, st.sf));
            st.since_bp += 1;
        }
        _ => {
            out.push(n);
            st.since_bp += 1;
        }
    }
}

/// A block of a list's entries as [`fold_item`] folds it, for a state
/// in: the state out, and the items it leaves, those it made (interline
/// glue, spaces resolved) kept outside the value table (a sweep may free
/// them).
struct BlockMemo {
    sf: i32,
    pd: i32,
    since_bp: usize,
    out: Vec<Result<ValueId, crate::api::Tree>>,
}

type BlockCache = std::sync::Mutex<phitex_defuse::FxHashMap<u64, std::sync::Arc<BlockMemo>>>;

fn block_cache() -> &'static BlockCache {
    static C: std::sync::OnceLock<BlockCache> = std::sync::OnceLock::new();
    C.get_or_init(BlockCache::default)
}

/// The accumulated entries `[lid, item]` of a list folded ([`fold_item`]
/// for those of list `lid`), memoized by blocks: a block ends after an
/// entry whose id is 0 mod 64 (content-defined, so an edit moves no
/// later block's bounds) and is keyed by the state in and its entries;
/// a block with a deletion (which reaches into what came before) is
/// folded every time. An edit folds again only the blocks it changed.
pub(crate) fn fold_entries(
    v: &Values,
    lid: i64,
    entries: &[ValueId],
    st: &mut FoldState,
    out: &mut Vec<ValueId>,
) {
    let push = |x: ValueId, st: &mut FoldState, out: &mut Vec<ValueId>| -> bool {
        let Value::List(xs) = &*v.get(x) else {
            return true;
        };
        if xs.len() != 2 || int_of(v, xs[0]) != lid {
            return true;
        }
        let deletes = nodes::tag(v, xs[1]) == T_DELETE;
        fold_item(v, lid, xs[1], st, out);
        !deletes
    };
    let mut at = 0;
    while at < entries.len() {
        let end = entries[at..]
            .iter()
            .position(|x| x.raw() % 64 == 0)
            .map_or(entries.len(), |k| at + k + 1);
        let block = &entries[at..end];
        at = end;
        let mut words = vec![
            v.instance(),
            u64::from_ne_bytes(lid.to_ne_bytes()),
            u64::from(st.sf.cast_unsigned()),
            u64::from(st.pd.cast_unsigned()),
            st.since_bp as u64,
        ];
        words.extend(block.iter().map(|x| x.raw()));
        let key = phitex_defuse::hash_words(13, &words);
        let hit = block_cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
            .cloned();
        if let Some(m) = hit {
            out.extend(m.out.iter().map(|x| match x {
                Ok(id) => *id,
                Err(t) => crate::api::import(v, t),
            }));
            (st.sf, st.pd, st.since_bp) = (m.sf, m.pd, m.since_bp);
            continue;
        }
        let o = out.len();
        let mut plain = true;
        for &x in block {
            plain &= push(x, st, out);
        }
        if !plain || out.len() < o {
            continue;
        }
        // the items the entries hold pass as they are; the rest were made
        let mut given = phitex_defuse::FxHashSet::default();
        for &x in block {
            if let Value::List(xs) = &*v.get(x)
                && let Some(&n) = xs.get(1)
            {
                given.insert(n);
                if let Value::List(ys) = &*v.get(n)
                    && let Some(&b) = ys.get(1)
                {
                    given.insert(b);
                }
            }
        }
        let memo = BlockMemo {
            sf: st.sf,
            pd: st.pd,
            since_bp: st.since_bp,
            out: out[o..]
                .iter()
                .map(|&x| {
                    if given.contains(&x) {
                        Ok(x)
                    } else {
                        Err(crate::api::export(v, x))
                    }
                })
                .collect(),
        };
        let mut c = block_cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if c.len() > 1 << 18 {
            c.clear();
        }
        c.insert(key, std::sync::Arc::new(memo));
    }
}

/// Whether `x` is a marker for the page builder's fold
/// (`T_PAGE_PARAMS`, `T_MARKS_AT`), not list material.
fn is_marker(v: &Values, x: ValueId) -> bool {
    matches!(
        nodes::tag(v, x),
        nodes::T_PAGE_PARAMS | nodes::T_MARKS_AT | nodes::T_WRITE_AT | nodes::T_FLUSH_AT
    )
}

/// A list's contents as its reader sees them ([`Run::list_fold`]).
pub(crate) struct ListFold {
    /// The items, transitions folded, waiting spaces and interline glue
    /// made.
    pub items: Vec<ValueId>,
    /// How many are still contributions (after the last `build_page`).
    pub since_bp: usize,
    /// The space factor at the end (§1034).
    pub sf: i32,
    /// The previous depth at the end (§679).
    pub pd: i32,
}

/// A transition setting the previous depth to `d`.
fn pd_set(v: &Values, d: i32) -> ValueId {
    v.list(vec![v.int(nodes::T_PD), v.int(i64::from(d))])
}

/// A transition setting the space factor to `s`.
fn sf_set(v: &Values, s: i32) -> ValueId {
    v.list(vec![v.int(nodes::T_SF), v.int(0), v.int(i64::from(s))])
}

/// The transition of a run's effect `sf` (not `SF_UNREAD`).
fn sf_value(v: &Values, sf: i32) -> ValueId {
    if sf >= 0 {
        sf_set(v, sf)
    } else {
        v.list(vec![v.int(nodes::T_SF), v.int(1), v.int(i64::from(-sf))])
    }
}

/// The glue of a space (§1041–§1044) from `\\spaceskip`, `\\xspaceskip`,
/// the font's dimensions 2, 3, 4 and 7, and the space factor.
fn space_node(ss: &Glue, xs: &Glue, d: [i32; 4], sf: i32, normal: bool) -> Node {
    let zero = |g: &Glue| g.width == 0 && g.stretch == 0 && g.shrink == 0;
    let font_glue = Glue::new(d[0], d[1], d[2]);
    if normal {
        return if zero(ss) {
            Node::glue(font_glue)
        } else {
            Node::param_glue(12, *ss)
        };
    }
    if sf >= 2000 && !zero(xs) {
        return Node::param_glue(13, *xs);
    }
    let mut g = if zero(ss) { font_glue } else { *ss };
    if sf >= 2000 {
        g.width += d[3];
    }
    g.stretch = phitex_typeset::arith::xn_over_d(g.stretch, sf, 1000).value;
    g.shrink = phitex_typeset::arith::xn_over_d(g.shrink, 1000, sf).value;
    Node::glue(g)
}

/// The glue node of a waiting space (`T_SPACE`) at space factor `sf`.
fn resolve_space(v: &Values, n: ValueId, sf: i32) -> ValueId {
    let xs = list_of(v, n);
    let ss = crate::val::glue_of(v, xs.get(1).copied());
    let xsk = crate::val::glue_of(v, xs.get(2).copied());
    let d = |k: usize| i32::try_from(crate::val::int_at(v, n, k)).unwrap_or(0);
    let node = space_node(&ss, &xsk, [d(3), d(4), d(5), d(6)], sf, sf == 1000);
    nodes::encode(v, &node, &Side::default())
}
