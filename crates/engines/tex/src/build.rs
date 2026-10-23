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

use crate::exec::LockWait as _;
use std::sync::{Arc, Mutex, OnceLock};

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
        if self.nest.is_empty() {
            return self.base_mode();
        }
        // (level 0's extra fields are the top level's here: not copied)
        let base = self.base_mode_with(false)?;
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
        self.base_mode_with(true)
    }

    /// [`Self::base_mode`], its extra fields left empty unless `with_extra`.
    fn base_mode_with(&mut self, with_extra: bool) -> R<Mode> {
        let k = self.scope_kind()?;
        let v = self.v;
        let ex = |p: &crate::run::KindFields| {
            (
                p.0,
                p.1,
                p.2,
                p.3,
                p.4,
                if with_extra { p.5.clone() } else { Vec::new() },
            )
        };
        let (group, sid, lid, km, depth, extra) = match k {
            Some(k) => match &self.kind_memo {
                Some((kk, p)) if *kk == k => ex(p),
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
                    let r = ex(&p);
                    self.kind_memo = Some((k, p));
                    r
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
                self.muts += 1;
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
        self.muts += 1;
        let nest = std::mem::take(&mut self.nest);
        self.abandon_levels(&nest);
        for l in nest {
            if self.claim_rest && l.owns {
                for x in crate::nest::into_values(self.v, l.list) {
                    self.append(0, x);
                }
            }
        }
    }

    /// The open lists at a fatal error (§1335): the main list keeps what
    /// it holds, up to the end (`T_ABORT`); a list open inside it, a
    /// paragraph's or a box's, ends with the job: none of it goes on the
    /// main list.
    pub(crate) fn end_fatal(&mut self) {
        self.muts += 1;
        let nest = std::mem::take(&mut self.nest);
        self.abandon_levels(&nest);
        for l in nest {
            if l.owns && l.lid == 0 {
                for x in crate::nest::into_values(self.v, l.list) {
                    self.append(0, x);
                }
            }
        }
        let v = self.v;
        self.append(0, v.list(vec![v.int(nodes::T_ABORT)]));
    }

    /// Leaves the run's one open group (sharing the list it is in) to
    /// the engine as a scope (`new_save_level`, §274): its kind is
    /// `[group, sid, list, mode, depth, parent kind, extra…]`, the
    /// following siblings' level 0 ([`Run::base_mode`]). It opens after
    /// the run's entry: the run's definitions at its base level lie
    /// outside it, the group's local ones are the scope's
    /// (`Ctx::define_in_scope`); its `\aftergroup` tokens the scope's
    /// `ag` list.
    pub(crate) fn leave_scope(&mut self) -> R<()> {
        let v = self.v;
        let base = self.base_mode()?;
        let parent = self.scope_kind()?.unwrap_or(ValueId::UNIT);
        // (what the records of the scope's definitions read, before the
        // group is left: [`Run::ready_records`])
        let defined = self.st.defined_records();
        for (b, &d) in defined.iter().enumerate() {
            if d {
                self.prepare_record(b)?;
            }
        }
        if self.nest.len() > 1 {
            self.leave_frames(&base, parent);
            return Ok(());
        }
        self.muts += 1;
        let Some(l) = self.nest.pop() else {
            return Ok(());
        };
        // (the group's local definitions are the scope's, the base
        // level's stay the run's: `Locality::Scope`)
        self.st
            .abandon_to_scope(v, l.save_base, l.def_base, l.open_stamp, self.rec_load);
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
            let f = self.ops.fields.get_at(v, self.cx.values_epoch(), "ag");
            let loc = self.loc_of(f);
            // (an `ag` list the run made at its base level, the enclosing
            // scope's, stays: the scope's is the field's second entry)
            if self.st.base_publishes(v, f, Some(loc), self.rec_load) {
                self.st
                    .define_scope_extra(loc, crate::runstate::Entry::raw(Some(v.list(xs))));
            } else {
                self.set("ag", v.list(xs), false);
                self.st.publish_in_scope(loc);
            }
        }
        if crate::exec::obs_flag!("PHITEX_GIVEBACK_TRACE") {
            let ds: Vec<String> = self
                .st
                .published(v, self.rec_load)
                .into_iter()
                .map(|(loc, x, l)| {
                    let f = self.loc_field(loc);
                    let mut r = v.render(x);
                    r.truncate(r.char_indices().nth(60).map_or(r.len(), |(i, _)| i));
                    format!("{} {l:?} {r}", crate::val::field_name(v, f))
                })
                .collect();
            eprintln!("leave_scope: node {} defs {ds:#?}", self.cx.node().raw());
        }
        // the scope opens after the run's entry: its base-level
        // definitions outside it, the group's inside
        self.effect = Effect::Scope(phitex_defuse::Brackets {
            closes: 0,
            opens: 1,
            before: false,
            kind: Some(kind),
            kind_pending: false,
        });
        Ok(())
    }

    /// Leaves the run's open groups (several, each sharing the list
    /// below it) to the engine as nested scopes, one frame per group,
    /// outermost first (`new_save_level` once per group, §274): frame
    /// `k`'s kind is group `k`'s, `[group, sid, list, mode, depth, parent
    /// kind, extra…]`, its parent kind frame `k - 1`'s (the scope the run
    /// runs in for frame 0), its sid its own; group `k`'s local
    /// definitions are frame `k`'s (`Ctx::define_in_scope_at`), its
    /// `\aftergroup` tokens frame `k`'s `ag` list. The run's definitions
    /// at its base level lie outside them all.
    fn leave_frames(&mut self, base: &crate::build::Mode, parent: ValueId) {
        let v = self.v;
        self.muts += 1;
        let levels = std::mem::take(&mut self.nest);
        let n = levels.len();
        let sid0 = i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes());
        let sid = |k: usize| sid0.wrapping_add(i64::try_from(k).unwrap_or(0));
        let mut kinds = Vec::with_capacity(n);
        let mut outer = parent;
        for (k, l) in levels.iter().enumerate() {
            let mut xs = vec![
                v.int(l.group),
                v.int(sid(k)),
                v.int(base.lid),
                v.int(i64::from(l.mode)),
                v.int(base.depth + 1 + i64::try_from(k).unwrap_or(0)),
                outer,
            ];
            xs.extend(l.extra.iter().copied());
            outer = v.list(xs);
            kinds.push(outer);
        }
        let spans: Vec<(usize, usize, u64)> = levels
            .iter()
            .map(|l| (l.save_base, l.def_base, l.open_stamp))
            .collect();
        self.st.abandon_to_frames(&spans);
        let f = self.ops.fields.get_at(v, self.cx.values_epoch(), "ag");
        let loc = self.loc_of(f);
        for (k, l) in levels.iter().enumerate() {
            if l.ag.is_empty() {
                continue;
            }
            let mut xs = vec![v.int(sid(k))];
            xs.extend(l.ag.iter().map(|t| t.encode(v)));
            self.st.define_frame_extra(
                loc,
                crate::runstate::Entry::raw(Some(v.list(xs))),
                u8::try_from(k).unwrap_or(u8::MAX - 2),
            );
        }
        self.cx.set_kinds(&kinds);
        self.effect = Effect::Scope(phitex_defuse::Brackets {
            closes: 0,
            opens: u8::try_from(n).unwrap_or(u8::MAX),
            before: false,
            kind: None,
            kind_pending: true,
        });
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
    /// [`Self::append`] of a node without marks or whatsits: kept native
    /// on a list of this run's ([`crate::nest::Item`]), encoded for the
    /// engine's.
    pub(crate) fn append_node(&mut self, lid: i64, node: Node) {
        if let Some(l) = self.level_of(lid) {
            l.list.push(crate::nest::Item::N(node));
            l.fold.since_bp += 1;
            return;
        }
        let x = nodes::encode(self.v, &node, &Side::default());
        self.append(lid, x);
    }

    /// [`Self::append`] of several items: to a list of the engine's
    /// scope, one contribution holding them all (a paragraph's lines).
    pub(crate) fn append_many(&mut self, lid: i64, nodes: Vec<ValueId>) {
        let v = self.v;
        if let Some(l) = self.level_of(lid) {
            for x in nodes {
                fold_item(v, lid, x, &mut l.fold, &mut l.list);
            }
            return;
        }
        if nodes.is_empty() {
            return;
        }
        if lid == 0 {
            if self.st.take_page_params() {
                self.put_page_params();
            }
            for &x in &nodes {
                if nodes::tag(v, x) == nodes::T_INS {
                    let n = u16::try_from(nodes::int_field(v, x, 1)).unwrap_or(0);
                    self.ins_pending.push(n);
                }
            }
        }
        let l = v.int(lid);
        let units: Vec<ValueId> = nodes
            .into_iter()
            .map(|x| v.seq_unit(v.list(vec![l, x])))
            .collect();
        let s = seq_balanced(v, &units);
        self.contribute("items", s);
    }

    pub(crate) fn append(&mut self, lid: i64, node: ValueId) {
        let v = self.v;
        if let Some(l) = self.level_of(lid) {
            fold_item(v, lid, node, &mut l.fold, &mut l.list);
            return;
        }
        if lid == 0 {
            // (the page parameters as they stand when the item arrives,
            // §987: for the breaker)
            if self.st.take_page_params() {
                self.put_page_params();
            }
            // (an insertion's class: its registers are read at the next
            // candidate, §1008, `Run::finish_candidate`)
            if nodes::tag(v, node) == nodes::T_INS {
                let n = u16::try_from(nodes::int_field(v, node, 1)).unwrap_or(0);
                self.ins_pending.push(n);
            }
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
        let v = self.v;
        if let Some(l) = self.level_of(lid) {
            return Ok((crate::nest::values(v, &mut l.list), l.list.len()));
        }
        let f = self.list_fold(lid)?;
        // the page builder's markers are no list material
        let v = self.v;
        let items = f.items.into_iter().filter(|&x| !is_marker(v, x)).collect();
        Ok((items, f.since_bp))
    }

    /// The items of list `lid` as [`Self::list_items`] has them, a list
    /// of this run's with its native nodes as they are.
    pub(crate) fn list_own_items(&mut self, lid: i64) -> R<Vec<crate::nest::Item>> {
        if let Some(l) = self.level_of(lid) {
            return Ok(l.list.clone());
        }
        Ok(crate::nest::of_values(self.list_items(lid)?.0))
    }

    /// [`Self::list_last`] for a reader of the last item's kind only (a
    /// glue, kern or penalty item whole, a box as `[tag, h]`, anything
    /// else as `[tag]`, [`crate::val::tail_projection`]): the main list
    /// as the pieces before left it, before this run appended to it,
    /// reads that projection of [`MAIN_LAST`], so a piece after another
    /// paragraph's last line reads only that a box ends the list.
    pub(crate) fn list_last_kind(&mut self, lid: i64) -> R<(Option<ValueId>, usize)> {
        if lid != 0 || self.level_of(lid).is_some() {
            return self.list_last(lid);
        }
        let f = self.list_fold_(lid, true)?;
        let v = self.v;
        let last = f.items.iter().rev().find(|&&x| !is_marker(v, x)).copied();
        Ok((last, f.since_bp))
    }

    /// The last item of list `lid` and the count [`Self::list_items`]
    /// gives, without copying a local list.
    pub(crate) fn list_last(&mut self, lid: i64) -> R<(Option<ValueId>, usize)> {
        let v = self.v;
        if let Some(l) = self.level_of(lid) {
            let n = l.list.len();
            return Ok((l.list.last_mut().map(|x| x.value(v)), n));
        }
        let (items, since) = self.list_items(lid)?;
        Ok((items.last().copied(), since))
    }

    /// [`Self::list_items`] with the space factor at the list's end: the
    /// list's transitions are folded left to right from 1000 (§1091,
    /// §1083), and each waiting space gets its glue from the factor where
    /// it stands (§1041–§1044).
    pub(crate) fn list_fold(&mut self, lid: i64) -> R<ListFold> {
        self.list_fold_(lid, false)
    }

    /// [`Self::list_fold`]; `kind_only`: the main list's last item before
    /// this run read as its kind only ([`Self::list_last_kind`]).
    #[allow(clippy::too_many_lines)]
    fn list_fold_(&mut self, lid: i64, kind_only: bool) -> R<ListFold> {
        let v = self.v;
        let f = v.field("items");
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
        if lid == 0 {
            // the main list as the pieces before left it: its last item
            // and the depth before this node ([`MAIN_LAST`],
            // [`MAIN_PD`]), not every item since the start; a deletion
            // of this run's reaching past that item reads the items
            // themselves, a real dependency
            let deletes = own.iter().any(|&s| {
                v.seq_to_vec(s).iter().any(|&x| {
                    let xs = list_of(v, x);
                    xs.len() == 2 && int_of(v, xs[0]) == 0 && nodes::tag(v, xs[1]) == T_DELETE
                })
            });
            // (a count that is a lower bound, `MainTail::since_at_least`:
            // the deletion may reach past the last item)
            let last = if kind_only && !deletes {
                self.cx.use_field_proj(v.field(MAIN_LAST), self.ops.tail_proj)?
            } else {
                self.eng_read(v.field(MAIN_LAST))?
            };
            let since = self.eng_read(v.field(MAIN_SINCE))?;
            let (last, since, at_least) = MainTail::of_last(v, last, since);
            if deletes && (since > 1 || at_least) {
                let all = self.cx.acc_range(f, None)?;
                fold_entries(v, 0, &v.seq_to_vec(all), &mut st, &mut out);
                // (this run's own items are in `all` only once it ends:
                // they follow)
            } else {
                out.extend(last);
                st.since_bp = since;
                // (the depth before this node only where this run's own
                // first box needs it for its interline glue, §679: a box
                // before any `\prevdepth` setting of its own; a reader of
                // the main list's last item reads nothing else)
                if own_box_needs_depth(v, &own) {
                    st.pd = self.main_pd_before()?;
                }
            }
        } else {
            // a list is this run's or the engine's (lid 0): no list is
            // opened by one node and read by another
            let all = self.cx.acc_range(f, None)?;
            fold_entries(v, lid, &v.seq_to_vec(all), &mut st, &mut out);
        }
        let mut push = |x: ValueId, out: &mut Vec<ValueId>| {
            let Some((l, item)) = entry_pair(v, x) else {
                return;
            };
            if int_of(v, l) != lid {
                return;
            }
            fold_item(v, lid, item, &mut st, out);
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
        if md.vlid == 0 {
            // (the depth before this node and this run's own items: no
            // other field read)
            let pd = self.main_pd_before()?;
            let own = self.own_main_items();
            let t = MainTail {
                pd,
                ..MainTail::START
            };
            return Ok(t.after(self.v, &own).pd);
        }
        Ok(self.list_fold(md.vlid)?.pd)
    }

    /// The main list's end after `raw` (this run's own main-list items)
    /// folded from the document's start, every item before read: where
    /// the count since the last candidate reached here as a lower bound
    /// only ([`MainTail::since_at_least`]) and a deletion reaches back.
    pub(crate) fn main_tail_full(&mut self, raw: &[ValueId]) -> R<MainTail> {
        let v = self.v;
        let all = self.cx.acc_range(v.field("items"), None)?;
        let mut out: Vec<ValueId> = Vec::new();
        let mut st = FoldState {
            sf: 1000,
            pd: IGNORE_DEPTH,
            since_bp: 0,
        };
        fold_entries(v, 0, &v.seq_to_vec(all), &mut st, &mut out);
        for &x in raw {
            fold_item(v, 0, x, &mut st, &mut out);
        }
        Ok(MainTail {
            last: out.iter().rev().find(|&&x| !is_marker(v, x)).copied(),
            since: st.since_bp,
            sf: st.sf,
            pd: st.pd,
        })
    }

    /// The main list's previous depth before this node ([`MAIN_PD`]).
    fn main_pd_before(&mut self) -> R<i32> {
        let v = self.v;
        Ok(self.eng_read(v.field(MAIN_PD))?.map_or(IGNORE_DEPTH, |x| {
            i32::try_from(int_of(v, x)).unwrap_or(IGNORE_DEPTH)
        }))
    }

    /// This run's own items on the main list, in order.
    fn own_main_items(&self) -> Vec<ValueId> {
        let v = self.v;
        let f = v.field("items");
        let mut out = Vec::new();
        for &(g, s) in &self.contribs {
            if g != f {
                continue;
            }
            for x in v.seq_to_vec(s) {
                // (the pair borrowed: no list copied per item)
                let pair = v
                    .with_list(x, |xs| (xs.len() == 2).then(|| (xs[0], xs[1])))
                    .flatten();
                if let Some((l, y)) = pair
                    && int_of(v, l) == 0
                {
                    out.push(y);
                }
            }
        }
        out
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
        let base = self.space_factor_listed(md)?;
        // (inside a character run's lookahead: its effect so far, §1034)
        Ok(match self.chars_live {
            Some((lid, e)) if lid == md.lid => sf_apply(base, e),
            _ => base,
        })
    }

    /// [`Run::space_factor`] of the list's items alone.
    fn space_factor_listed(&mut self, md: &Mode) -> R<i32> {
        if let Some(l) = self.level_of(md.lid) {
            return Ok(l.fold.sf);
        }
        if md.lid == 0 {
            // (the space factor before this node and this run's own
            // items: no other field read)
            let v = self.v;
            let sf = self
                .eng_read(v.field(MAIN_SF))?
                .map_or(1000, |x| i32::try_from(int_of(v, x)).unwrap_or(1000));
            let own = self.own_main_items();
            let t = MainTail {
                sf,
                ..MainTail::START
            };
            return Ok(t.after(v, &own).sf);
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
        self.finish_candidate_at(true)
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
                    self.muts += 1;
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
                // (layer 3: the lines as a node of their own where the
                // piece ends here, `crate::parbreak`)
                if self.break_as_node(md) {
                    self.place_break(md, items)?;
                } else {
                    self.line_break(md, items, false)?;
                }
            }
            self.normal_paragraph()?;
        }
        Ok(())
    }

    /// The paragraph's level when groups opened inside the paragraph
    /// are still open above it (`abc{def\\par}`): the innermost level,
    /// under groups that share its list only.
    pub(crate) fn par_level_under_groups(&self) -> Option<usize> {
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
    pub(crate) fn end_graf_under_groups(&mut self, k: usize) -> R<Vec<crate::nest::Item>> {
        let (mode, lid) = if let Some(b) = k.checked_sub(1) {
            (self.nest[b].mode, self.nest[b].lid)
        } else {
            let base = self.base_mode()?;
            (base.m, base.lid)
        };
        self.muts += 1;
        let l = self.nest.remove(k);
        for g in &mut self.nest[k..] {
            g.mode = mode;
            g.lid = lid;
        }
        self.muts += 1;
        self.last_par = l.par.unwrap_or((0, 0));
        Ok(l.list)
    }

    /// The fonts the nodes use.
    pub(crate) fn font_view(&mut self, list: &[Node]) -> R<crate::fonts::View> {
        self.font_view_with(list, &[])
    }

    /// The view of fonts `ids` ([`Self::add_font_metrics`] each).
    pub(crate) fn view_of(&mut self, ids: std::collections::BTreeSet<u32>) -> R<crate::fonts::View> {
        let mut view = crate::fonts::View::default();
        for f in ids {
            self.add_font_metrics(&mut view, f)?;
        }
        Ok(view)
    }

    /// [`Self::font_view`] with fonts `extra` too (a page's sealed lines',
    /// `crate::pages`).
    pub(crate) fn font_view_with(&mut self, list: &[Node], extra: &[u32]) -> R<crate::fonts::View> {
        fn walk(l: &[Node], ids: &mut std::collections::BTreeSet<u32>) {
            // (a run of characters in one font is one insert)
            let mut last = None;
            for n in l {
                match n {
                    Node::Char { font, .. } => {
                        if last != Some(*font) {
                            ids.insert(*font);
                            last = Some(*font);
                        }
                    }
                    Node::Lig(g) => {
                        if last != Some(g.font) {
                            ids.insert(g.font);
                            last = Some(g.font);
                        }
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
        ids.extend(extra.iter().copied());
        let mut view = crate::fonts::View::default();
        for f in ids {
            self.add_font_metrics(&mut view, f)?;
        }
        Ok(view)
    }

    /// [`Self::add_font`] without the `\fontdimen`s: what measuring,
    /// breaking and shipping a list need (the metrics, the hyphen
    /// character, the name); only math reads the parameters through
    /// the view (`Fonts::param`), and its view is made by `add_font`.
    pub(crate) fn add_font_metrics(&mut self, view: &mut crate::fonts::View, f: u32) -> R<()> {
        if view.fonts.contains_key(f) || f == 0 {
            return Ok(());
        }
        let info = self.font_get(f, crate::fields::FontCell::Info)?;
        let Some(i) = crate::fonts::info(self.v, info) else {
            return Ok(());
        };
        let Some(l) = i
            .file
            .and_then(|file| crate::fonts::load(self.v, file, i.s))
        else {
            return Ok(());
        };
        let hc = self.font_int(f, crate::fields::FontCell::Hc)?;
        view.hyphen.insert(f, hc);
        view.names.insert(f, (i.name, i.size));
        view.fonts.insert(f, (l, Vec::new()));
        Ok(())
    }

    /// [`Self::add_font_metrics`] with `\\fontdimen`s `ks` read (the
    /// others 0, their number the font's): a view whose reader asks
    /// only those of the font.
    pub(crate) fn add_font_dimens(&mut self, view: &mut crate::fonts::View, f: u32, ks: &[i32]) -> R<()> {
        if view.fonts.contains_key(f) || f == 0 {
            return Ok(());
        }
        let info = self.font_get(f, crate::fields::FontCell::Info)?;
        let Some(i) = crate::fonts::info(self.v, info) else {
            return Ok(());
        };
        let Some(l) = i
            .file
            .and_then(|file| crate::fonts::load(self.v, file, i.s))
        else {
            return Ok(());
        };
        let n = self.font_int(f, crate::fields::FontCell::Fdn)?;
        let mut params = vec![0; usize::try_from(n).unwrap_or(0)];
        for &k in ks {
            if let Some(p) = usize::try_from(k - 1).ok().and_then(|j| params.get_mut(j)) {
                *p = self.font_int(f, crate::fields::FontCell::Fd(k))?;
            }
        }
        let hc = self.font_int(f, crate::fields::FontCell::Hc)?;
        view.hyphen.insert(f, hc);
        view.names.insert(f, (i.name, i.size));
        view.fonts.insert(f, (l, params));
        Ok(())
    }

    pub(crate) fn add_font(&mut self, view: &mut crate::fonts::View, f: u32) -> R<()> {
        if view.fonts.contains_key(f) || f == 0 {
            return Ok(());
        }
        let Some(e) = self.font_entry(f)? else {
            return Ok(());
        };
        view.hyphen.insert(f, e.hc);
        view.names.insert(f, e.name.clone());
        view.fonts.insert(f, (e.loaded.clone(), e.params.clone()));
        Ok(())
    }

    /// Font `f` as the state has it now: its metrics, `\fontdimen`s,
    /// `\hyphenchar` and name, decoded once while no slot of the run's
    /// table changes ([`crate::runstate::State::any_gen`]).
    pub(crate) fn font_entry(&mut self, f: u32) -> R<Option<std::rc::Rc<crate::fonts::Entry>>> {
        let g = self.st.any_gen();
        if let Some((eg, e)) = self.font_memo.get(&f)
            && *eg == g
            && self.interp.is_none()
        {
            return Ok(e.clone());
        }
        let info = self.font_get(f, crate::fields::FontCell::Info)?;
        let e = match crate::fonts::info(self.v, info) {
            Some(i) => match i
                .file
                .and_then(|file| crate::fonts::load(self.v, file, i.s))
            {
                Some(l) => {
                    let n = self.font_int(f, crate::fields::FontCell::Fdn)?;
                    let mut params = Vec::new();
                    for k in 1..=n {
                        params.push(self.font_int(f, crate::fields::FontCell::Fd(k))?);
                    }
                    let hc = self.font_int(f, crate::fields::FontCell::Hc)?;
                    Some(std::rc::Rc::new(crate::fonts::Entry {
                        loaded: l,
                        params,
                        hc,
                        name: (i.name.clone(), i.size),
                    }))
                }
                None => None,
            },
            None => None,
        };
        // (the reads above define nothing: the generation stands)
        if self.interp.is_none() {
            let g = self.st.any_gen();
            self.font_memo.insert(f, (g, e.clone()));
        }
        Ok(e)
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
        items: Vec<crate::nest::Item>,
        display: bool,
    ) -> R<Option<BoxNode>> {
        let t0 = crate::prof::start();
        crate::prof::paragraph();
        let r = self.line_break_(md, items, display);
        crate::prof::add(crate::prof::LB, t0);
        r
    }

    #[allow(clippy::too_many_lines)]
    fn line_break_(
        &mut self,
        md: &Mode,
        items: Vec<crate::nest::Item>,
        display: bool,
    ) -> R<Option<BoxNode>> {
        let v = self.v;
        let t0 = crate::prof::start();
        let mut side = Side::default();
        // (collected in place: the items' buffer becomes the list's; its
        // fonts, and its characters for the hyphenator's `\lccode`s, found
        // on the way)
        let mut ids = std::collections::BTreeSet::new();
        let mut seen = [false; 256];
        let mut font_was = u32::MAX;
        let list: List = items
            .into_iter()
            .filter_map(|x| {
                let n = match x {
                    crate::nest::Item::N(n) => n,
                    crate::nest::Item::V(x) => nodes::decode(v, x, &mut side)?,
                };
                match &n {
                    Node::Char { font, ch } => {
                        if let Some(s) = seen.get_mut(usize::from(*ch)) {
                            *s = true;
                        }
                        if *font != font_was {
                            ids.insert(*font);
                            font_was = *font;
                        }
                    }
                    // (no character in it)
                    Node::Glue(g) if g.leader.is_none() => {}
                    Node::Kern(..)
                    | Node::Penalty(_)
                    | Node::Math(..)
                    | Node::Rule(_)
                    | Node::Whatsit(..)
                    | Node::Mark(_)
                    | Node::Language { .. } => {}
                    n => fonts_chars_of(std::slice::from_ref(n), &mut ids, &mut seen),
                }
                Some(n)
            })
            .collect();
        crate::prof::add(crate::prof::LB_DEC, t0);
        let t0 = crate::prof::start();
        // (\lccode of the characters the hyphenator can meet: those of
        // the list, `seen`; a character not in it is never asked for)
        let view = self.view_of(ids)?;
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
        let mut lc = [0u16; 256];
        for (c, x) in lc.iter_mut().enumerate() {
            if seen[c] {
                *x = u16::try_from(i64::from(self.code(0, c)?)).unwrap_or(0);
            }
        }
        let uc_hyph = self.ipar(Par::uchyph)?;
        let hyphen = &view.hyphen;
        let lcf = |c: u16| lc.get(usize::from(c)).copied().unwrap_or(0);
        let hcf = |f: u32| hyphen.get(&f).copied().unwrap_or(-1);
        let mut hy = phitex_typeset::hyph::TexHyphenator {
            patterns: &patterns,
            lc_code: &lcf,
            uc_hyph,
            hyphen_char: &hcf,
        };
        crate::prof::add(crate::prof::LB_PARAMS, t0);
        let t0 = crate::prof::start();
        let para = phitex_typeset::linebreak::line_break(list, display, &params, &view, &mut hy);
        crate::prof::add(crate::prof::LB_TS, t0);
        let t0 = crate::prof::start();
        // (the last line, for a display that interrupts the paragraph:
        // moved out once encoded)
        let mut just_box = None;
        let last_k = para.lines.len().wrapping_sub(1);
        // (the interline glue parameters in force for every line, §679:
        // nothing between the lines changes them)
        let bs = crate::val::glue_value(v, &self.gpar(Par::baselineskip)?);
        let ls = crate::val::glue_value(v, &self.gpar(Par::lineskip)?);
        let lsl = v.int(i64::from(self.ipar(Par::lineskiplimit)?));
        let tv = v.int(nodes::T_VBOX);
        let mut out = Vec::with_capacity(para.lines.len() * 2);
        for (k, mut line) in para.lines.into_iter().enumerate() {
            self.box_report(true, line.packed.report, true);
            let (list, seen, line_fonts) = nodes::encode_line(v, &line.packed.node.list, &side);
            let b = nodes::encode_box_with(v, &line.packed.node, list);
            // (its fonts from its nodes, for its unit's shape: not walked
            // again through its value, `crate::nodes::unit_shape`)
            if seen == 0
                && let Some(ids) = line_fonts
            {
                let f = {
                    // (one value a set of fonts: the lines share few)
                    let ids = ids.as_slice();
                    let k = phitex_defuse::hash_words(0, &ids.iter().map(|&f| u64::from(f)).collect::<Vec<_>>());
                    match self.font_sets.get(&k) {
                        Some((s, x)) if s.as_slice() == ids => *x,
                        _ => {
                            let x = v.list(ids.iter().map(|&f| v.int(i64::from(f))).collect());
                            self.font_sets.insert(k, (ids.to_vec(), x));
                            x
                        }
                    }
                };
                self.line_fonts.insert(b, f);
            }
            out.push(v.list(vec![tv, b, bs, ls, lsl]));
            for a in &line.packed.adjustments {
                out.push(nodes::encode(v, a, &side));
            }
            if let Some(p) = line.penalty {
                out.push(nodes::penalty_node(v, p));
            }
            if display && k == last_k {
                just_box = Some(line.packed.node);
            } else {
                // (its marks and whatsits numbered as its value's
                // decoding numbers them, its shallow boxes' lists
                // decoded: the line kept as its value decodes)
                let (mut marks, mut whats) = (Vec::new(), Vec::new());
                let ok = seen == 0
                    || nodes::localize(
                        v,
                        &mut line.packed.node.list,
                        &side,
                        &mut marks,
                        &mut whats,
                    );
                nodes::made_stat(if seen == 0 {
                    0
                } else if ok {
                    5
                } else {
                    6
                });
                let kept = if ok {
                    nodes::keep_made(list, line.packed.node.list, marks, whats)
                } else {
                    Some(line.packed.node.list)
                };
                if let Some(l) = kept {
                    phitex_typeset::linebreak::recycle_line(l);
                }
            }
        }
        // the paragraph's lines: one contribution
        self.append_many(md.vlid, out);
        self.set_prev_graf(md, para.prev_graf);
        crate::prof::add(crate::prof::LB_COMMIT, t0);
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
        if let Some(p) = cache.locked().get(&key) {
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
        cache.locked().insert(key, p.clone());
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

    /// The location of box register `n` (spec-runstate §2: a fixed one
    /// for 0–255, the spill's by its field past them).
    pub(crate) fn box_loc(&mut self, n: i64) -> crate::runstate::Loc {
        if let Some(i) = usize::try_from(n).ok().filter(|&i| i < 256) {
            crate::runstate::Loc::try_from(crate::fields::BOX0 + i).unwrap_or(0)
        } else {
            let f = self.box_field(n);
            self.loc_of(f)
        }
    }

    /// The box in register `n` (`None`: void).
    pub(crate) fn box_reg(&mut self, n: i32) -> R<Option<ValueId>> {
        let x = {
            let l = self.box_loc(i64::from(n));
            self.get_loc(l)?
        };
        let v = self.v;
        Ok(x.and_then(|x| {
            v.with_list(x, |xs| xs.get(1).copied().filter(|&b| b != ValueId::UNIT))
                .flatten()
        }))
    }

    /// `eq_define` of a box register (§1077): its level is the group's.
    pub(crate) fn set_box_reg(&mut self, md: &Mode, n: i64, b: Option<ValueId>, global: bool) {
        let v = self.v;
        let lvl = if global { 0 } else { md.depth };
        let val = v.list(vec![v.int(lvl), b.unwrap_or(ValueId::UNIT)]);
        let l = self.box_loc(n);
        self.set_loc(l, crate::runstate::Entry::raw(Some(val)), global);
    }

    /// A box register changed "at the same level" (§1079 `box(n):=null`,
    /// §1247): the binding keeps its level and is replaced where it was
    /// made, so no group saves it and the groups opened above that level
    /// keep the change at their close ([`Run::modify_at`]).
    pub(crate) fn modify_box_reg(&mut self, n: i32, b: Option<ValueId>) -> R<()> {
        let v = self.v;
        let l = self.box_loc(i64::from(n));
        let x = self.get_loc(l)?;
        let lvl = x.map_or(0, |x| crate::val::int_at(v, x, 0));
        let val = v.list(vec![v.int(lvl), b.unwrap_or(ValueId::UNIT)]);
        let base = self.base_mode()?.depth;
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
        let t0 = std::time::Instant::now();
        let r = self.begin_box_(md, ctx);
        phitex_pdf::phase::add(22, t0);
        r
    }

    #[allow(clippy::too_many_lines)]
    fn begin_box_(&mut self, md: &Mode, ctx: i64) -> R<()> {
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
                    let (last, since) = self.list_last_kind(md.lid)?;
                    let ok = last.is_some_and(|x| nodes::tag(v, x) == nodes::T_BOX)
                        && !(md.m == 1 && since == 0);
                    // (the box itself only where it is taken)
                    let last = if ok { self.list_last(md.lid)?.0 } else { None };
                    if ok {
                        let m = nodes::marker(v, T_DELETE);
                        self.append(md.lid, m);
                        // (its shift set to 0 in the value itself: the box
                        // is not decoded and encoded again whole)
                        last.map(|x| with_shift(v, x, 0))
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
        self.box_end_in(md, ctx, b, true)
    }

    /// [`Run::box_end`]; without `candidate`, the caller looks for a
    /// page after what it appends next (§1076: the box, then its
    /// adjustments, then `build_page`).
    pub(crate) fn box_end_in(
        &mut self,
        md: &Mode,
        ctx: i64,
        b: Option<ValueId>,
        candidate: bool,
    ) -> R<()> {
        let v = self.v;
        if ctx < BOX_FLAG {
            // §1076
            let Some(b) = b else { return Ok(()) };
            let b = with_shift(v, b, i32::try_from(ctx).unwrap_or(0));
            if md.m.abs() == 1 {
                self.append_to_vlist(md, b)?;
                if md.m > 0 && candidate {
                    self.finish_candidate()?;
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
        let (last, since) = self.list_last_kind(md.lid)?;
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
        let t0 = std::time::Instant::now();
        let r = self.unpackage_(md);
        phitex_pdf::phase::add(21, t0);
        r
    }

    fn unpackage_(&mut self, md: &Mode) -> R<()> {
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
        // (a finisher's sealed line opened: its own list, read now; the
        // lines in an opened box stay sealed, `crate::pages::seal`)
        let b = if list_of(v, b).get(9).is_some_and(|&l| nodes::is_sealed(v, l)) {
            self.unseal(b)
        } else {
            b
        };
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
            let c = self.font_int(f, crate::fields::FontCell::Hc)?;
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
            let x = nodes::encode(v, &Node::Disc(Box::new(d)), &Side::default());
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
                    p = Node::Box(Box::new(bx));
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
        let t0 = std::time::Instant::now();
        let r = self.handle_right_brace_(md);
        phitex_pdf::phase::add(20, t0);
        r
    }

    #[allow(clippy::too_many_lines)]
    fn handle_right_brace_(&mut self, md: &Mode) -> R<()> {
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
                let items = self.list_own_items(md.lid)?;
                let mut side = Side::default();
                let mut kept = Vec::with_capacity(items.len());
                let list: List = items
                    .into_iter()
                    .filter_map(|y| match y {
                        crate::nest::Item::N(n) => {
                            kept.push(nodes::encode(v, &n, &Side::default()));
                            Some(n)
                        }
                        crate::nest::Item::V(y) => {
                            // (the items measured: a box's list stays encoded)
                            let n = nodes::decode_shallow(v, y, &mut side)?;
                            kept.push(y);
                            Some(n)
                        }
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
                    self.box_end_in(&outer, ctx, Some(b), false)?;
                }
                if adjusted {
                    for a in &packed.adjustments {
                        let y = nodes::encode(v, a, &side);
                        self.append(outer.vlid, y);
                    }
                }
                // §1076: `if mode>0 then build_page`, after the adjustments
                if ctx < BOX_FLAG && outer.m > 0 && self.effect != Effect::Close {
                    self.finish_candidate()?;
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
                    Node::Ins(Box::new(phitex_typeset::node::Insert {
                        number: u16::try_from(n).unwrap_or(0),
                        height: p.node.height + p.node.depth,
                        depth: d,
                        split_top_skip: q,
                        float_cost: f,
                        list: p.node.list,
                    }))
                } else {
                    Node::Adjust(p.node.list)
                };
                let y = nodes::encode(v, &node, &side);
                self.append(outer.lid, y);
                self.finish_candidate()
            }
            group::DISC => self.build_discretionary(md),
            group::OUTPUT => self.output_end(md),
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
            self.muts += 1;
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
        let d = Node::Disc(Box::new(Disc {
            pre,
            post,
            replace: list,
        }));
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
        let t0 = crate::prof::start();
        self.chars_eff = SF_UNREAD;
        let r = self.char_run_(md, cancel);
        self.chars_live = None;
        crate::prof::add(crate::prof::CHAR, t0);
        r
    }

    #[allow(clippy::too_many_lines)]
    fn char_run_(&mut self, md: &Mode, cancel: bool) -> R<()> {
        let v = self.v;
        let f = u32::try_from(self.cint(crate::fields::cell::FONT)?).unwrap_or(0);
        // (the view of the worker's last character run, emptied)
        let mut view = CHAR_VIEW.with(std::cell::Cell::take).unwrap_or_default();
        view.clear();
        self.add_font_metrics(&mut view, f)?;
        // the space factor is never read: the run's effect on it so far
        // (`sf`: SF_UNREAD none, n >= 0 set to n, -s §1034's update by s)
        // goes on the list as a transition before each space that needs
        // the factor, and at the end; the list's reader folds them
        let mut sf = SF_UNREAD;
        // (the buffer of the worker's last character run, reused)
        let mut out: Vec<crate::nest::Item> = CHAR_BUF.with(std::cell::Cell::take);
        out.clear();
        // transitions and waiting spaces, each before `out[k]`
        let mut raws: Vec<(usize, ValueId)> = Vec::new();
        let mut cancel_boundary = cancel;
        // (the space parameters, read once: no command of this loop
        // changes them)
        let mut space = SpaceMemo::default();
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
                    if sf != SF_UNREAD {
                        raws.push((out.len(), sf_value(v, sf)));
                        sf = SF_UNREAD;
                    }
                    self.flush_chars(md.lid, &mut out, std::mem::take(&mut raws));
                    self.chars_eff = SF_UNREAD;
                    self.get_x_token();
                    self.ck()?;
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
                        let g = self.space_glue_memo(f, sf, normal, &mut space)?;
                        out.push(crate::nest::Item::N(g));
                    } else {
                        if sf != SF_UNREAD {
                            raws.push((out.len(), sf_value(v, sf)));
                            self.chars_eff = sf_compose(self.chars_eff, sf);
                            sf = SF_UNREAD;
                        }
                        let p = if let Some(p) = space.waiting {
                            p
                        } else {
                            let p = self.waiting_space(f)?;
                            space.waiting = Some(p);
                            p
                        };
                        raws.push((out.len(), p));
                    }
                }
                _ => {
                    self.back_cur();
                    break;
                }
            }
            // big_switch
            if !self.fast_char() {
                if !self.has_input() {
                    break;
                }
                // (what the expansion reads of the list, its tail and the
                // space factor, §1034: the run's nodes so far on it)
                if sf != SF_UNREAD {
                    raws.push((out.len(), sf_value(v, sf)));
                    sf = SF_UNREAD;
                }
                self.flush_chars(md.lid, &mut out, std::mem::take(&mut raws));
                self.chars_eff = SF_UNREAD;
                self.get_x_token();
                self.ck()?;
            }
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
        let te = crate::prof::start();
        self.flush_chars(md.lid, &mut out, raws);
        CHAR_BUF.with(|c| c.set(out));
        CHAR_VIEW.with(|c| c.set(Some(view)));
        crate::prof::add(crate::prof::CHAR_ENC, te);
        Ok(())
    }

    /// A character run's nodes and transitions (each transition before
    /// `out[k]`) put on list `lid`, `out` emptied.
    fn flush_chars(&mut self, lid: i64, out: &mut Vec<crate::nest::Item>, raws: Vec<(usize, ValueId)>) {
        let v = self.v;
        let mut raws = raws.into_iter().peekable();
        if let Some(l) = self.level_of(lid) {
            // (the run's own list: the nodes go on it native, each stretch
            // between two transitions in one go)
            l.list.reserve(out.len() + raws.len());
            let mut nodes = out.drain(..);
            let mut k = 0;
            for (at, r) in raws {
                if at > k {
                    l.list.extend(nodes.by_ref().take(at - k));
                    l.fold.since_bp += at - k;
                    k = at;
                }
                fold_item(v, lid, r, &mut l.fold, &mut l.list);
            }
            let rest = nodes.len();
            l.list.extend(nodes);
            l.fold.since_bp += rest;
        } else {
            for (k, n) in out.drain(..).enumerate() {
                while let Some((_, r)) = raws.next_if(|&(at, _)| at == k) {
                    self.append(lid, r);
                }
                match n {
                    crate::nest::Item::N(n) => self.append_node(lid, n),
                    crate::nest::Item::V(x) => self.append(lid, x),
                }
            }
            for (_, r) in raws {
                self.append(lid, r);
            }
        }
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

    /// [`Self::space_glue`] with the parameters read once a character
    /// run (`m`).
    fn space_glue_memo(&mut self, f: u32, sf: i32, normal: bool, m: &mut SpaceMemo) -> R<Node> {
        let ss = if let Some(g) = m.ss {
            g
        } else {
            let g = self.gpar(Par::spaceskip)?;
            m.ss = Some(g);
            g
        };
        let zero = |g: &Glue| g.width == 0 && g.stretch == 0 && g.shrink == 0;
        if normal && !zero(&ss) {
            return Ok(Node::param_glue(12, ss));
        }
        let xs = if normal {
            Glue::ZERO
        } else if let Some(g) = m.xs {
            g
        } else {
            let g = self.gpar(Par::xspaceskip)?;
            m.xs = Some(g);
            g
        };
        let fi = i64::from(f);
        let mut d = [0; 4];
        for (k, n) in [2, 3, 4, 7].into_iter().enumerate() {
            if (k < 3 && zero(&ss)) || (k == 3 && sf >= 2000) {
                d[k] = if let Some(x) = m.d[k] {
                    x
                } else {
                    let x = self.font_dimen(fi, n)?;
                    m.d[k] = Some(x);
                    x
                };
            }
        }
        Ok(space_node(&ss, &xs, d, sf, normal))
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
    #[inline]
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

    /// [`Self::main_loop`]'s run of plain letters: while the next
    /// character of the line on top is a letter or other character
    /// (by the catcodes cached) with no ligature or kern after `cur_l`,
    /// not after the hyphen character, not the false boundary character
    /// and in the font, it is read, its space factor adjusted, and it
    /// goes on `out` as the steps of §1036–§1039 put it; the first that
    /// is not is left unread.
    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn plain_letters(
        &mut self,
        md: &Mode,
        f: u32,
        view: &crate::fonts::View,
        (hyphen_char, false_bchar): (i32, Option<u32>),
        sf: &mut i32,
        out: &mut Vec<crate::nest::Item>,
        (cur_l, cur_q): (&mut Option<u32>, &mut usize),
    ) -> R<()> {
        if self.cat_gen != self.st.cat_gen() {
            return Ok(());
        }
        let Some(l) = self.input.levels.last() else {
            return Ok(());
        };
        if l.kind != crate::input::kind::LINE {
            return Ok(());
        }
        let li = l.a as usize;
        let Some((loaded, _)) = view.fonts.get(f) else {
            return Ok(());
        };
        let tfm = &loaded.tfm;
        let chars = &loaded.at.chars;
        // (whether the list ends with the hyphen character: a
        // discretionary is §1035's to insert)
        let mut after_hyphen =
            matches!(out.last(), Some(crate::nest::Item::N(Node::Char { ch, .. })) if i32::from(*ch) == hyphen_char);
        loop {
            let Some(cl) = *cur_l else { return Ok(()) };
            if after_hyphen {
                return Ok(());
            }
            let line = &self.input.lines[li];
            if line.endch == -2 {
                return Ok(());
            }
            let Some(&c) = line.buf.get(line.loc) else {
                return Ok(());
            };
            if c == 0 || c > 255 {
                return Ok(());
            }
            let cat = self.cat_cache[c as usize];
            if cat != cmd::LETTER && cat != cmd::OTHER_CHAR {
                return Ok(());
            }
            if false_bchar == Some(c) || chars.get(c as usize).is_none_or(Option::is_none) {
                return Ok(());
            }
            let (Ok(l8), Ok(r8)) = (u8::try_from(cl), u8::try_from(c)) else {
                return Ok(());
            };
            if tfm.lookup(Some(l8), Some(r8)).is_some() {
                return Ok(());
            }
            let line = &mut self.input.lines[li];
            line.loc += 1;
            line.state = crate::input::MID_LINE;
            self.adjust_sf(md, c, sf)?;
            *cur_q = out.len();
            *cur_l = Some(c);
            let ch = u16::try_from(c).unwrap_or(0);
            after_hyphen = i32::from(ch) == hyphen_char;
            out.push(crate::nest::Item::N(Node::Char { font: f, ch }));
        }
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
        out: &mut Vec<crate::nest::Item>,
        cancel_boundary: &mut bool,
    ) -> R<bool> {
        type Item = LigItem;
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
        let Some((loaded, _)) = view.fonts.get(f) else {
            // the null font: every character is missing
            return Ok(false);
        };
        let tfm = &loaded.tfm;
        let mut bchar: Option<u32> = tfm.bchar().map(u32::from);
        let false_bchar: Option<u32> = tfm.false_bchar().map(u32::from);
        let hyphen_char = view.hyphen.get(&f).copied().unwrap_or(-1);
        let para = md.m > 0;
        self.adjust_sf(md, ch, sf)?;
        let mut lig_stack = std::mem::take(&mut self.lig_buf);
        lig_stack.clear();
        lig_stack.push(Item {
            ch,
            lig: false,
            ptr: None,
        });
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
                      out: &mut Vec<crate::nest::Item>,
                      lig_present: &mut bool,
                      lft_hit: &mut bool,
                      rt_hit: &mut bool,
                      ins_disc: &mut bool,
                      stack_empty: bool| {
            // §1035
            let Some(cl) = cur_l else { return };
            if out.len() > cur_q
                && matches!(out.last(), Some(crate::nest::Item::N(Node::Char { ch, .. })) if i32::from(*ch) == hyphen_char)
            {
                *ins_disc = true;
            }
            if *lig_present {
                let original: Vec<u16> = out[cur_q..]
                    .iter()
                    .filter_map(|n| match n {
                        crate::nest::Item::N(Node::Char { ch, .. }) => Some(*ch),
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
                out.push(crate::nest::Item::N(Node::Lig(lg)));
                *lig_present = false;
            }
            if *ins_disc {
                *ins_disc = false;
                if para {
                    out.push(crate::nest::Item::N(Node::Disc(Box::default())));
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
                    // (mostly a plain character: nothing to wrap up, no
                    // call)
                    let busy = cur_l.is_some()
                        && (lig_present
                            || ins_disc
                            || (out.len() > cur_q
                                && matches!(out.last(), Some(crate::nest::Item::N(Node::Char { ch, .. })) if i32::from(*ch) == hyphen_char)));
                    if busy {
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
                    }
                    at = L::Move;
                }
                L::Move => {
                    // §1036
                    let Some(top) = lig_stack.last().copied() else {
                        self.lig_buf = lig_stack;
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
                    out.push(crate::nest::Item::N(Node::Char {
                        font: f,
                        ch: u16::try_from(input).unwrap_or(0),
                    }));
                    lig_stack.pop();
                    at = L::Lookahead;
                }
                L::Lookahead => {
                    // (the word's plain letters straight from the line, as
                    // the steps below take them one at a time: §1038's
                    // lookahead, §1039's lookup finding nothing, nothing
                    // to wrap up, §1036's move; the first that is not
                    // plain is left to them)
                    if lig_stack.is_empty() && !lig_present && !ins_disc {
                        self.plain_letters(md, f, view, (hyphen_char, false_bchar), sf, out, (&mut cur_l, &mut cur_q))?;
                    }
                    // §1038; at the end of the node's text the next token
                    // is not a character (characters are one node)
                    let mut got: Option<u32> = None;
                    if self.fast_char() {
                        // (a letter, other character or space)
                        if self.cur.cmd != cmd::SPACER {
                            got = Some(self.cur.chr.cast_unsigned());
                        }
                    } else if !self.has_input() {
                        input_done = true;
                    } else if self.next_ck()? {
                        // (§1038: the characters so far are on the list, the
                        // last one its tail, the space factor after it:
                        // what an expansion here reads, [`Run::chars_live`])
                        self.chars_live = Some((md.lid, sf_compose(self.chars_eff, *sf)));
                        if !matches!(
                            self.cur.cmd,
                            cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN
                        ) {
                            self.x_token();
                        }
                        let r = self.ck().and_then(|()| {
                            if self.cur.cmd == cmd::CHAR_NUM {
                                self.scan_char_num().map(Some)
                            } else {
                                Ok(None)
                            }
                        });
                        self.chars_live = None;
                        // (§1038: a `\char` scanned is the character
                        // `cur_val`, whatever token ended its number: the
                        // scan leaves that token in `cur`, a space it took
                        // or the one it put back)
                        match r? {
                            Some(n) => got = Some(u32::try_from(n).unwrap_or(0)),
                            None => match self.cur.cmd {
                                cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN => {
                                    got = Some(u32::try_from(self.cur.chr).unwrap_or(0));
                                }
                                cmd::NO_BOUNDARY => bchar = None,
                                _ => {}
                            },
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
                        out.push(crate::nest::Item::N(Node::Char {
                            font: f,
                            ch: u16::try_from(c).unwrap_or(0),
                        }));
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
                        // (§1035–§1037 where nothing is to wrap up: the
                        // input's character, alone on the stack, goes on
                        // the list and the next is looked ahead)
                        if let [top] = lig_stack.as_slice()
                            && !top.lig
                            && cur_l.is_some()
                            && !lig_present
                            && !ins_disc
                            && !(out.len() > cur_q
                                && matches!(out.last(), Some(crate::nest::Item::N(Node::Char { ch, .. })) if i32::from(*ch) == hyphen_char))
                            && view.exists(f, top.ch)
                        {
                            let c = top.ch;
                            lig_stack.clear();
                            cur_q = out.len();
                            cur_l = Some(c);
                            out.push(crate::nest::Item::N(Node::Char {
                                font: f,
                                ch: u16::try_from(c).unwrap_or(0),
                            }));
                            at = L::Lookahead;
                            continue;
                        }
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
                        out.push(crate::nest::Item::N(Node::Kern(loaded.at.char_kern(step), KernKind::Normal)));
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
            let info = self.font_get(f, crate::fields::FontCell::Info)?;
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
        // (`mode = 0` in a `\write`, §1370: no tail; a character run's
        // lookahead: its last character, §1038)
        let last = if self.mode_zero || self.chars_live.is_some_and(|(l, _)| l == md.lid) {
            None
        } else {
            self.list_last_kind(md.lid)?.0
        };
        let v = self.v;
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
        let m = v.list(vec![v.int(nodes::T_END), b, g, p]);
        self.append(0, m);
        // nothing after `\end` is read
        self.muts += 1;
        self.claim_rest = true;
        self.input = crate::input::Input::new();
        // §1054: `build_page`
        self.finish_candidate()
    }

    /// `\vsplit` (§977): the box split off register `n` at height `h`.
    fn vsplit(&mut self, n: i32, h: i32) -> R<Option<ValueId>> {
        // (§977, e-TeX's `vsplit_init`: the split marks are cleared first,
        // whatever the box holds, a void box's split leaving none)
        let Some(b) = self.box_reg(n)? else {
            crate::etex::split_marks(self, &[], &Side::default())?;
            return Ok(None);
        };
        let v = self.v;
        let mut side = Side::default();
        // (the box's top list only: the break measures its boxes and
        // never looks into them, §970-§977; they are put back encoded)
        let Some(bx) = nodes::decode_box_top(v, b, &mut side) else {
            crate::etex::split_marks(self, &[], &Side::default())?;
            return Ok(None);
        };
        if bx.kind != BoxKind::V {
            crate::etex::split_marks(self, &[], &Side::default())?;
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

/// A list entry `[list id, item]`, borrowed.
fn entry_pair(v: &phitex_defuse::Values, x: ValueId) -> Option<(ValueId, ValueId)> {
    v.with_list(x, |xs| (xs.len() == 2).then(|| (xs[0], xs[1])))
        .flatten()
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
    s.lazy.extend(b.lazy);
    s.lazy_box.extend(b.lazy_box);
    s
}

/// The fonts of list `l`'s characters into `ids` and their codes marked
/// in `seen` (for the hyphenator's `\lccode`s), nested lists included:
/// what a paragraph's view and its hyphenation need of it.
fn fonts_chars_of(l: &[Node], ids: &mut std::collections::BTreeSet<u32>, seen: &mut [bool; 256]) {
    fn mark(seen: &mut [bool; 256], c: u16) {
        if let Some(s) = seen.get_mut(usize::from(c)) {
            *s = true;
        }
    }
    let mut last = None;
    for n in l {
        match n {
            Node::Char { font, ch } => {
                mark(seen, *ch);
                if last != Some(*font) {
                    ids.insert(*font);
                    last = Some(*font);
                }
            }
            Node::Lig(g) => {
                mark(seen, g.ch);
                for &c in &g.original {
                    mark(seen, c);
                }
                if last != Some(g.font) {
                    ids.insert(g.font);
                    last = Some(g.font);
                }
            }
            Node::Box(b) => fonts_chars_of(&b.list, ids, seen),
            Node::Unset(u) => fonts_chars_of(&u.list, ids, seen),
            Node::Disc(d) => {
                fonts_chars_of(&d.pre, ids, seen);
                fonts_chars_of(&d.post, ids, seen);
                fonts_chars_of(&d.replace, ids, seen);
            }
            Node::Ins(i) => fonts_chars_of(&i.list, ids, seen),
            Node::Adjust(a) => fonts_chars_of(a, ids, seen),
            Node::Glue(g) => {
                if let Some(l) = &g.leader {
                    fonts_chars_of(std::slice::from_ref(l), ids, seen);
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn _unused(_: ValueId) -> Option<Glue> {
    let _ = glue_of;
    None
}

thread_local! {
    /// A character run's node buffer, kept for the worker's next run
    /// ([`Run::char_run`]).
    static CHAR_BUF: std::cell::Cell<Vec<crate::nest::Item>> = const { std::cell::Cell::new(Vec::new()) };
    /// Its font view, kept the same way.
    static CHAR_VIEW: std::cell::Cell<Option<crate::fonts::View>> = const { std::cell::Cell::new(None) };
}

/// The space factor not read yet in a run of characters.
const SF_UNREAD: i32 = i32::MIN;

/// Two space factor effects one after the other (each `SF_UNREAD`
/// none, `n >= 0` set to `n`, `-s` §1034's update by an `sf_code` `s`
/// above 1000): after an update the factor is 1000 or more, so a second
/// update sets it.
pub(crate) fn sf_compose(a: i32, b: i32) -> i32 {
    if b == SF_UNREAD {
        a
    } else if b >= 0 || a == SF_UNREAD {
        b
    } else if (0..1000).contains(&a) {
        1000
    } else {
        -b
    }
}

/// Effect `e` ([`sf_compose`]) on the factor `base`.
pub(crate) fn sf_apply(base: i32, e: i32) -> i32 {
    if e == SF_UNREAD {
        base
    } else if e >= 0 {
        e
    } else if base >= 1000 {
        -e
    } else {
        1000
    }
}

/// The space parameters a character run read ([`Run::char_run`]):
/// `\\spaceskip`, `\\xspaceskip`, the font's dimensions 2, 3, 4 and 7,
/// and a waiting space's value.
#[derive(Default)]
struct SpaceMemo {
    ss: Option<Glue>,
    xs: Option<Glue>,
    d: [Option<i32>; 4],
    waiting: Option<ValueId>,
}

/// An entry of the main loop's `lig_stack` (§1034): a character, whether
/// it is a ligature's, and the character a ligature replaced.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LigItem {
    ch: u32,
    lig: bool,
    ptr: Option<u32>,
}

/// The state of a list's fold: the space factor (§1034), the previous
/// depth (§679), and the items since the last `build_page` marker.
/// A list's fold state at its start (§215: the depth ignored).
pub(crate) const IGNORE_DEPTH_STATE: FoldState = FoldState {
    sf: 1000,
    pd: IGNORE_DEPTH,
    since_bp: 0,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    out: &mut impl FoldOut,
) {
    let _ = lid;
    match nodes::tag(v, n) {
        T_DELETE => {
            if st.since_bp > 0 {
                // the last node, seen through the page builder's markers
                out.delete_last(v);
                st.since_bp -= 1;
            }
        }
        // the page builder's markers (page parameters, a marks read): kept
        // in place for its fold, invisible to the list's readers
        nodes::T_PAGE_PARAMS
        | nodes::T_WRITE_AT
        | nodes::T_FONT_AT
        | nodes::T_PDFW_AT
        | nodes::T_SET_PAGE
        | nodes::T_INS_PARAMS => out.push_v(n),
        T_BUILD_PAGE => {
            st.since_bp = 0;
        }
        T_PD => st.pd = i32::try_from(crate::val::int_at(v, n, 1)).unwrap_or(0),
        T_VBOX => {
            // (the fields borrowed: no list copied per line)
            let ys: [Option<ValueId>; 5] = v
                .with_list(n, |xs| std::array::from_fn(|k| xs.get(k).copied()))
                .unwrap_or_else(|| {
                    let xs = list_of(v, n);
                    std::array::from_fn(|k| xs.get(k).copied())
                });
            let b = ys[1].unwrap_or(ValueId::UNIT);
            let (bs, ls) = line_glues(v, ys[2], ys[3]);
            let lsl = ys[4].map_or(0, |x| i32::try_from(int_of(v, x)).unwrap_or(0));
            let ht = nodes::box_dim(v, b, 3);
            if let Some(g) = phitex_typeset::linebreak::interline_glue(st.pd, ht, bs, ls, lsl) {
                out.push_node(v, g);
                st.since_bp += 1;
            }
            out.push_v(b);
            st.since_bp += 1;
            st.pd = nodes::box_dim(v, b, 2);
        }
        T_SF => {
            let k = crate::val::int_at(v, n, 1);
            let s = i32::try_from(crate::val::int_at(v, n, 2)).unwrap_or(1000);
            st.sf = if k == 0 || st.sf >= 1000 { s } else { 1000 };
        }
        T_SPACE => {
            out.push_v(resolve_space(v, n, st.sf));
            st.since_bp += 1;
        }
        _ => {
            out.push_v(n);
            st.since_bp += 1;
        }
    }
}

/// The `\\baselineskip` and `\\lineskip` of a line's item, decoded
/// once for the lines that share them (a paragraph's: the same values).
fn line_glues(v: &Values, bs: Option<ValueId>, ls: Option<ValueId>) -> (Glue, Glue) {
    type Last = Option<(u64, Option<ValueId>, Option<ValueId>, Glue, Glue)>;
    thread_local! {
        static LAST: std::cell::Cell<Last> = const { std::cell::Cell::new(None) };
    }
    if let Some((i, b, l, gb, gl)) = LAST.get()
        && i == v.instance()
        && b == bs
        && l == ls
    {
        return (gb, gl);
    }
    let (gb, gl) = (crate::val::glue_of(v, bs), crate::val::glue_of(v, ls));
    LAST.set(Some((v.instance(), bs, ls, gb, gl)));
    (gb, gl)
}

/// What [`fold_item`] folds into: a list of values (an engine list's
/// reader), or a list of this run's items ([`crate::nest::Item`]).
pub(crate) trait FoldOut {
    fn push_v(&mut self, x: ValueId);
    /// A node the fold made (interline glue): its value.
    fn push_node(&mut self, v: &Values, n: Node) {
        self.push_v(nodes::encode(v, &n, &Side::default()));
    }
    /// Removes the last item that is not a page builder's marker.
    fn delete_last(&mut self, v: &Values);
}

/// What [`MainTail::after`] folds into: the items as the fold leaves
/// them, a node it made kept native until it is the last (only the last
/// item and the count are read).
#[derive(Default)]
struct TailOut(Vec<Result<ValueId, phitex_typeset::node::GlueNode>>);

impl FoldOut for TailOut {
    fn push_v(&mut self, x: ValueId) {
        self.0.push(Ok(x));
    }
    fn push_node(&mut self, v: &Values, n: Node) {
        match n {
            Node::Glue(g) => self.0.push(Err(g)),
            n => self.0.push(Ok(nodes::encode(v, &n, &Side::default()))),
        }
    }
    fn delete_last(&mut self, v: &Values) {
        if let Some(i) = self.0.iter().rposition(|x| x.as_ref().is_ok_and(|&x| !is_marker(v, x)) || x.is_err()) {
            drop(self.0.remove(i));
        }
    }
}

impl FoldOut for Vec<ValueId> {
    fn push_v(&mut self, x: ValueId) {
        self.push(x);
    }
    fn delete_last(&mut self, v: &Values) {
        if let Some(i) = self.iter().rposition(|&x| !is_marker(v, x)) {
            self.remove(i);
        }
    }
}

impl FoldOut for Vec<crate::nest::Item> {
    fn push_v(&mut self, x: ValueId) {
        self.push(crate::nest::Item::V(x));
    }
    fn delete_last(&mut self, v: &Values) {
        if let Some(i) = self.iter().rposition(|x| !x.is_marker(v)) {
            self.remove(i);
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
        let Some((l, item)) = entry_pair(v, x) else {
            return true;
        };
        if int_of(v, l) != lid {
            return true;
        }
        let deletes = nodes::tag(v, item) == T_DELETE;
        fold_item(v, lid, item, st, out);
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
        let hit = block_cache().locked().get(&key).cloned();
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
            if let Some(n) = v.with_list(x, |xs| xs.get(1).copied()).flatten() {
                given.insert(n);
                if let Some(b) = v.with_list(n, |ys| ys.get(1).copied()).flatten() {
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
        let mut c = block_cache().locked();
        if c.len() > 1 << 18 {
            c.clear();
        }
        c.insert(key, std::sync::Arc::new(memo));
    }
}

/// The concatenation of the sequences `xs`, as a balanced tree (the
/// engine's sequences are associative on ids: any shape is the same
/// value).
fn seq_balanced(v: &Values, xs: &[ValueId]) -> ValueId {
    match xs {
        [] => ValueId::NIL,
        [x] => *x,
        _ => {
            let (a, b) = xs.split_at(xs.len() / 2);
            v.seq_concat(seq_balanced(v, a), seq_balanced(v, b))
        }
    }
}

/// Whether `x` is a marker for the page builder's fold
/// (`T_PAGE_PARAMS`), not list material.
pub(crate) fn is_marker(v: &Values, x: ValueId) -> bool {
    matches!(
        nodes::tag(v, x),
        nodes::T_PAGE_PARAMS
            | nodes::T_WRITE_AT
            | nodes::T_FONT_AT
            | nodes::T_PDFW_AT
            | nodes::T_SET_PAGE
            | nodes::T_INS_PARAMS
    )
}

/// What a piece leaves the next piece of the main vertical list
/// (spec-redesign §1, exec row; no list tail as a value): three fields
/// each piece defines in its definition delta where its items change
/// them ([`Run::define_main_tail`]) and a later piece reads on first
/// touch, only the one it needs: the previous depth (`\\prevdepth`,
/// §679), the space factor (§1034), and the last item with the count of
/// contributions after the page builder's last call (`[item, since]`:
/// what `\\lastskip`, `\\lastkern`, `\\lastpenalty`, `\\unskip`,
/// `\\unkern`, `\\unpenalty`, `\\lastbox` see at a piece's start,
/// §424, §1080, §1105).
pub(crate) const MAIN_PD: &str = "vlist:pd";
pub(crate) const MAIN_SF: &str = "vlist:sf";
pub(crate) const MAIN_LAST: &str = "vlist:last";
/// The main list's contributions since the last candidate, as the pieces
/// before left it: apart from [`MAIN_LAST`], so that a piece whose items
/// change one and not the other (a blank line's candidate) reads
/// neither.
pub(crate) const MAIN_SINCE: &str = "vlist:since";

/// The main list's end as a piece's reader sees it ([`MAIN_PD`],
/// [`MAIN_SF`], [`MAIN_LAST`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MainTail {
    pub last: Option<ValueId>,
    pub since: usize,
    pub sf: i32,
    pub pd: i32,
}

impl MainTail {
    /// The document's start (§215, §1083: a vertical list begins with
    /// the depth ignored, the space factor 1000).
    pub(crate) const START: MainTail = MainTail {
        last: None,
        since: 0,
        sf: 1000,
        pd: IGNORE_DEPTH,
    };

    /// A start no document has: what a piece's items leave unchanged
    /// from it is the piece's own where it differs from [`Self::START`]'s
    /// ([`Run::define_main_tail`]).
    pub(crate) fn probe(v: &Values, n: usize) -> MainTail {
        MainTail {
            last: Some(v.list(vec![v.int(-0x5e57), v.int(i64::try_from(n).unwrap_or(0))])),
            since: n + 1,
            sf: 7777,
            pd: 7_654_321,
        }
    }

    /// The main list's end after `items` (a piece's own main-list items)
    /// folded onto this one.
    pub(crate) fn after(self, v: &Values, items: &[ValueId]) -> MainTail {
        let mut out = TailOut(Vec::with_capacity(2 * items.len() + 1));
        out.0.extend(self.last.map(Ok));
        let mut st = FoldState {
            sf: self.sf,
            pd: self.pd,
            since_bp: self.since,
        };
        for &x in items {
            fold_item(v, 0, x, &mut st, &mut out);
        }
        MainTail {
            last: out.0.iter().rev().find_map(|x| match x {
                Ok(x) if is_marker(v, *x) => None,
                Ok(x) => Some(*x),
                Err(g) => Some(nodes::encode(v, &Node::Glue(g.clone()), &Side::default())),
            }),
            since: st.since_bp,
            sf: st.sf,
            pd: st.pd,
        }
    }

    /// [`MAIN_LAST`]'s value.
    pub(crate) fn last_value(self) -> ValueId {
        self.last.unwrap_or(ValueId::UNIT)
    }

    /// [`MAIN_SINCE`]'s value.
    pub(crate) fn since_value(self, v: &Values) -> ValueId {
        v.int(i64::try_from(self.since).unwrap_or(0))
    }

    /// [`MAIN_SINCE`]'s value where a piece's items only add `d` to the
    /// count before it, no candidate among them (§1060: the glue after
    /// the paragraph before): at least `d`, the count before not read.
    /// Its readers ask whether it is zero (§424, §1080, §1105); a
    /// deletion that may reach further folds the list
    /// ([`Run::list_fold`]).
    pub(crate) fn since_at_least(v: &Values, d: usize) -> ValueId {
        v.int(-i64::try_from(d).unwrap_or(0))
    }

    /// [`MAIN_LAST`] and [`MAIN_SINCE`] read (`None`: the document's
    /// start): the last item, the count, and whether the count is a
    /// lower bound only ([`Self::since_at_least`]).
    pub(crate) fn of_last(
        v: &Values,
        last: Option<ValueId>,
        since: Option<ValueId>,
    ) -> (Option<ValueId>, usize, bool) {
        let n = since.map_or(0, |n| int_of(v, n));
        (
            last.filter(|&i| i != ValueId::UNIT),
            usize::try_from(n.unsigned_abs()).unwrap_or(0),
            n < 0,
        )
    }
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
    /// The previous depth at the end (§679). Of the main list, only where
    /// the run's own items set or need it (`\prevdepth` of the main list
    /// is [`Run::prev_depth`]'s, which reads the depth before itself).
    pub pd: i32,
}

/// Whether a run's own main-list entries (`items` units) put a box on
/// the list before any previous-depth setting of their own: its
/// interline glue is made from the depth before the run (§679).
fn own_box_needs_depth(v: &Values, own: &[ValueId]) -> bool {
    for &s in own {
        for x in v.seq_to_vec(s) {
            let Some((l, item)) = entry_pair(v, x) else {
                continue;
            };
            if int_of(v, l) != 0 {
                continue;
            }
            match nodes::tag(v, item) {
                nodes::T_VBOX => return true,
                nodes::T_PD => return false,
                _ => {}
            }
        }
    }
    false
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
