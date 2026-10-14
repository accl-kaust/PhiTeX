//! The page builder (§980–§1028), `\shipout` (§638) and the DVI file
//! (§583–§645).
//!
//! The page builder is one fold over the main vertical list: the
//! `Breaks` node reads every contribution to list 0 (the lines and the
//! vertical material every paragraph region contributed, in document
//! order) and runs §994–§1017's arithmetic over them once, choosing
//! every page break. Each page it fires is a child `PageStep` node with
//! the page's material as its argument: `\box255`, the insertions, the
//! marks, the held-over insertions and `\outputpenalty`; the step runs
//! the output routine inline on its own nest (`OutPage` §1025 opened at
//! its start, `output_end` §1026 at its `}`) and ships the page. The
//! steps read nothing of each other: a step defines nothing to the
//! engine; what its routine defines is the chain's delta, which the
//! writer folds into the loop-backs `pgst:<j>` the next round's steps
//! read (round one's guess: no change), with `\count0..9` in
//! `pgdef:<j>`.
//!
//! What a routine leaves for the next page (material `\unvbox`ed back,
//! `\vsize` changed) is a loop-back the fold reads from the previous
//! round (`pagefx:<j>`: held inserts and the leftover list; page
//! parameters the routine sets are planned, branch para-vsize); round
//! one assumes none. Marks read by document nodes are planned to go
//! through a per-node loop-back `marks:before:<N>` (branch para-marks).

use std::collections::VecDeque;

use phitex_defuse::{
    Ctx, Effect, Field, Locality, MayDefine, Operation, Stop, Value, ValueId, Values,
};
use phitex_typeset::glue::Glue;
use phitex_typeset::node::{List, Node};
use phitex_typeset::pack::{PackParams, Spec};
use phitex_typeset::page::{InsertClass, Outcome, PageBuilder, PageParams};

use crate::build::Mode;
use crate::fields::Par;
use crate::nodes::{self, Side};
use crate::ops::{Shared, client_only, ops, run_node};
use crate::run::{R, Run};
use crate::val::{group, int_of, list_of, str_of, toks_of};

/// The most pages a build may hold loop-back effects for.
pub(crate) const MAX_PAGES: usize = 4096;

impl Run<'_, '_> {
    /// `ship_out` (§638–§640): the page goes to the `pages` accumulator
    /// with its `\count0..9` and fonts; its `\write`s and `\openout`s
    /// happen now (§1366–§1374).
    pub(crate) fn ship_out(&mut self, b: ValueId) -> R<()> {
        let v = self.v;
        let mut counts = [0i32; 10];
        for (k, c) in counts.iter_mut().enumerate() {
            *c = self.reg_int(0, k)?;
        }
        let mut j = 9;
        while j > 0 && counts[j] == 0 {
            j -= 1;
        }
        let mut s = String::from("[");
        for (k, c) in counts.iter().enumerate().take(j + 1) {
            s.push_str(&c.to_string());
            if k < j {
                s.push('.');
            }
        }
        self.log("ship", &s)?;
        if crate::exec::env_flag!("PHITEX_AUX_TRACE") {
            eprintln!(
                "aux-trace: ship {s} in page step {} node {} op {:?}",
                self.page_index(),
                self.cx.node().raw(),
                self.cx.op()
            );
        }
        let mut side = Side::default();
        let Some(Node::Box(bx)) = nodes::decode(v, b, &mut side) else {
            return Ok(());
        };
        let pos = crate::pdfprim::positions(self, &bx)?;
        // what the PDF writer makes of the page's pdfTeX whatsits
        let mut pdf_data = Vec::new();
        // §1366: the whatsits in the order `hlist_out`/`vlist_out` meet them
        for (wi, w) in side.whats.clone().into_iter().enumerate() {
            let xs = list_of(v, w);
            let kind = xs.first().map(|&k| str_of(v, k)).unwrap_or_default();
            match kind.as_str() {
                "write" => {
                    let k = i32::try_from(xs.get(1).map_or(0, |&y| int_of(v, y))).unwrap_or(0);
                    let toks = toks_of(v, xs.get(2).copied());
                    self.write_out(k, &toks)?;
                }
                "open" => {
                    let n = xs.get(1).map_or(0, |&y| int_of(v, y));
                    let name = xs.get(2).map(|&y| str_of(v, y)).unwrap_or_default();
                    self.open_out(n, &name)?;
                }
                "close" => {
                    let n = xs.get(1).map_or(0, |&y| int_of(v, y));
                    self.set(&format!("write:{n}"), ValueId::UNIT, true);
                }
                k => {
                    let at = u32::try_from(wi).ok().and_then(|i| pos.at.get(&i).copied());
                    crate::pdfprim::at_shipout(self, k, at, &pos)?;
                    if pos.pdf
                        && let Some(d) = crate::pdfprim::writer_data(self, &xs)?
                    {
                        let h = i64::try_from(wi).unwrap_or(0);
                        pdf_data.push(v.list(vec![v.int(h), d]));
                    }
                }
            }
        }
        let page_box = Node::Box(bx);
        let view = self.font_view(std::slice::from_ref(&page_box))?;
        let mut fonts = Vec::new();
        let mut ids: Vec<u32> = view.fonts.keys().copied().collect();
        ids.sort_unstable();
        for &f in &ids {
            let info = self.get(&format!("fontinfo:{f}"))?;
            let Some(i) = crate::fonts::info(v, info) else {
                continue;
            };
            let Some(file) = i.file else { continue };
            fonts.push(v.list(vec![
                v.int(i64::from(f)),
                v.str(&i.name),
                file,
                v.int(i64::from(i.s)),
            ]));
        }
        let cs = v.list(counts.iter().map(|&c| v.int(i64::from(c))).collect());
        let pdf = self.pdf_page_params(pos.pdf)?;
        let page = v.list(vec![b, cs, v.list(fonts), pdf, v.list(pdf_data)]);
        let shown = match &page_box {
            Node::Box(bx) if pos.pdf => crate::pdfout::ship(self, page, bx, &side, &ids)?,
            _ => dvi_shown(v, page),
        };
        let su = v.seq_unit(page);
        self.contribute("pages", su);
        let h = v.seq_unit(v.int(i64::from_ne_bytes(shown.to_ne_bytes())));
        self.contribute("pagehash", h);
        self.log("shipend", "]")?;
        self.set_cell(crate::fields::cell::DEADCYCLES, v.int(0), true);
        Ok(())
    }

    /// The parameters pdfTeX reads at each page (pdfTeX §750–§757), with
    /// whether the file is PDF and the colour stacks made so far.
    fn pdf_page_params(&mut self, pdf: bool) -> R<ValueId> {
        let v = self.v;
        let mut xs = vec![v.int(i64::from(pdf))];
        for p in [
            "pdfhorigin",
            "pdfvorigin",
            "pdfpagewidth",
            "pdfpageheight",
            "hoffset",
            "voffset",
            "pdfcompresslevel",
            "pdfdecimaldigits",
            "pdfminorversion",
            "pdfmajorversion",
            "mag",
        ] {
            xs.push(v.int(i64::from(self.param(p)?)));
        }
        xs.push(
            self.get_loc(crate::fields::cell::PDF_COLORSTACKS)?
                .unwrap_or(ValueId::UNIT),
        );
        for p in ["pdfobjcompresslevel", "pdfgentounicode"] {
            xs.push(v.int(i64::from(self.param(p)?)));
        }
        for p in ["pdfpageattr", "pdfpagesattr", "pdfpageresources"] {
            let toks = toks_of(v, self.get(&format!("param:{p}"))?);
            let mut s = String::new();
            self.show_toks_into(&mut s, &toks)?;
            xs.push(v.str(&s));
        }
        Ok(v.list(xs))
    }

    pub(crate) fn insert_class(&mut self, n: u16) -> R<InsertClass> {
        let b = self.box_reg(i32::from(n))?;
        let v = self.v;
        Ok(InsertClass {
            box_height_plus_depth: b.map(|b| nodes::box_dim(v, b, 3) + nodes::box_dim(v, b, 2)),
            box_is_hbox: b.is_some_and(|b| nodes::box_is_h(v, b)),
            count: self.reg_int(0, n)?,
            dimen: self.reg_int(1, n)?,
            skip: crate::val::glue_of(v, self.reg(2, i32::from(n))?),
        })
    }

    /// `\pagegoal` and the other page quantities (§421) read by the
    /// text: the fold has not run where a text node is, so the empty
    /// page's values (§987): `max_dimen` for the goal, 0 otherwise.
    #[allow(clippy::unnecessary_wraps)]
    pub(crate) fn page_dimen(&mut self, m: i32) -> R<i32> {
        self.diag(
            false,
            "\\pagegoal and its kin read from the text: the empty page's values",
        );
        Ok(if m == 0 { MAX_DIMEN } else { 0 })
    }

    /// `pageplan`: the main vertical list folded once (each page's part `pp:items:<j>`), the
    /// insertion classes it uses (`pp:classes`), whether the routine is
    /// empty (`pp:any`); then §994–§1017's arithmetic over the whole list
    /// choosing every break (what each routine left, `pagefx:<j>`, taken
    /// from the previous round), giving each page's start
    /// (`pp:start:<j>`) and the `marks:before:<N>` values; then one `brk`
    /// node per page, which re-folds its own page and fires its step:
    /// the pages' material is made and the steps run in parallel.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn page_plan(&mut self) -> R<()> {
        use crate::planinc::PageRec;
        use std::sync::Arc;
        let v = self.v;
        let tm = crate::exec::env_flag!("PHITEX_PLAN_TIME");
        let t0 = std::time::Instant::now();
        let epoch = self.cx.values_epoch();
        let all = self.cx.acc_range(v.field("items"), None)?;
        let t_acc = t0.elapsed();
        let mut prev = crate::planinc::take(v, epoch);
        let (folded, change) =
            crate::planinc::fold(v, all, prev.as_mut().map(|p| std::mem::take(&mut p.items)));
        let prev = prev.filter(|_| change.is_some());
        let items: &[ValueId] = &folded.items;
        let t1 = t0.elapsed();
        let marker = self.page_params_marker()?;
        // the insertion classes on the list, read here (before any page
        // step): a `brk` reads nothing a page step may define (counted
        // again only over what the edit changed)
        let ins_count =
            |xs: &[ValueId], sign: isize, c: &mut std::collections::BTreeMap<u16, isize>| {
                for &x in xs {
                    if nodes::tag(v, x) == nodes::T_INS
                        && let Some(Node::Ins(i)) = nodes::decode(v, x, &mut Side::default())
                    {
                        *c.entry(i.number).or_default() += sign;
                    }
                }
            };
        let mut ins = std::collections::BTreeMap::new();
        match (&prev, &change) {
            (Some(p), Some(c)) => {
                ins.clone_from(&p.ins);
                ins_count(&c.removed, -1, &mut ins);
                ins_count(&items[c.pre..items.len() - c.suf], 1, &mut ins);
                ins.retain(|_, n| *n > 0);
            }
            _ => ins_count(items, 1, &mut ins),
        }
        let mut table = Vec::new();
        for &n in ins.keys() {
            let c = self.insert_class(n)?;
            table.push(v.list(vec![
                v.int(i64::from(n)),
                v.int(c.box_height_plus_depth.map_or(-1, i64::from)),
                v.int(i64::from(c.box_is_hbox)),
                v.int(i64::from(c.count)),
                v.int(i64::from(c.dimen)),
                crate::val::glue_value(v, &c.skip),
            ]));
        }
        let table_v = v.list(table.clone());
        let output = self.par(Par::output)?;
        let any = toks_of(v, output).is_empty();
        // the previous run's pages, where they still hold: the items
        // both lists share at their start and their end
        let reusable = prev
            .as_ref()
            .filter(|p| p.table == table_v && p.marker == marker);
        let (pre, suf, old_len) = change
            .as_ref()
            .map_or((0, 0, 0), |c| (c.pre, c.suf, c.old_len));
        let d = crate::planinc::delta(items.len(), old_len);
        let mut recs: Vec<Arc<PageRec>> = Vec::new();
        let mut start = PageStart::FIRST;
        let mut marks = MarksState::default();
        let mut published = phitex_defuse::FxHashSet::default();
        let mut n_starts = 1usize;
        // (each page's `\\topmark` and `\\botmark` before it fires,
        // §1012: from the plan, so a step never reads its predecessor's)
        let mut before: Vec<ValueId> = Vec::new();
        let mut ended = false;
        let mut plan: Vec<ValueId> = Vec::new();
        let mut flushplan: Vec<ValueId> = Vec::new();
        let mut flushed = phitex_defuse::FxHashSet::default();
        let mut fonts_at: Vec<ValueId> = Vec::new();
        let mut objs_at: Vec<ValueId> = Vec::new();
        let mut probes: Vec<(i64, usize)> = Vec::new();
        let mut folds = 0usize;
        let (mut tfx, mut tfold) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
        // nothing to reuse: the pages folded in parallel from predicted
        // starts, each taken below only where its start is the real one
        let tp = std::time::Instant::now();
        let mut ahead = if reusable.is_none() {
            ahead_folds(v, items, table_v, marker)
        } else {
            Vec::new()
        };
        let tahead = tp.elapsed();
        let mut j = 0;
        while !ended && j < MAX_PAGES {
            let tf = std::time::Instant::now();
            let fx = if j > 0 {
                self.page_effects(j - 1)?
            } else {
                Vec::new()
            };
            tfx += tf.elapsed();
            // the old page at this start, when what it read is unchanged
            let old = reusable.and_then(|p| {
                let pick = |idx: usize, ok: &dyn Fn(&PageRec) -> bool| {
                    p.by_idx.get(&idx).and_then(|ks| {
                        ks.iter().map(|&k| &p.pages[k]).find(|r| {
                            ok(r) && r.fx == fx && crate::planinc::same_start(&r.start, &start)
                        })
                    })
                };
                pick(start.idx, &|r: &PageRec| r.reach <= pre)
                    .cloned()
                    .or_else(|| {
                        let o = start.idx.checked_add_signed(-d)?;
                        (o >= old_len - suf)
                            .then(|| pick(o, &|_: &PageRec| true))
                            .flatten()
                            .map(|r| {
                                if d == 0 {
                                    r.clone()
                                } else {
                                    Arc::new(r.shifted(d))
                                }
                            })
                    })
            });
            let ahead_hit = ahead.get_mut(j).and_then(|a| {
                (a.start.idx == start.idx
                    && crate::planinc::same_start(&a.start, &start)
                    && a.fx == fx)
                    .then(|| a.rec.take())
                    .flatten()
            });
            let rec = if let Some(r) = old {
                r
            } else if let Some(r) = ahead_hit {
                r
            } else {
                folds += 1;
                let tq = std::time::Instant::now();
                let out = plan_fold(v, items, table_v, marker, &start, fx.clone());
                tfold += tq.elapsed();
                Arc::new(page_rec(v, items, &start, fx, out))
            };
            // the job-written files' order: an immediate write's entry
            // where its marker is, page `j` where it fires (`wplan`)
            // (and the fonts loaded before page `j` fires: `fplan`)
            // (and the objects written after the page before it:
            // `oplan`)
            let (mut fl, mut ol) = (Vec::new(), Vec::new());
            for &w in &rec.writes {
                match nodes::tag(v, w) {
                    nodes::T_FONT_AT => fl.push(v.int(crate::val::int_at(v, w, 1))),
                    nodes::T_PDFW_AT => ol.push(v.int(crate::val::int_at(v, w, 1))),
                    nodes::T_PDFQ_AT => probes.push((crate::val::int_at(v, w, 1), j)),
                    _ => plan.push(w),
                }
            }
            fonts_at.push(v.list(fl));
            objs_at.push(v.list(ol));
            // the flush points met folding this page: each follows the
            // penalty the page before broke at (§1005 fires there, so
            // the marker after it opens this page): the state after
            // page j-1 (none before the first page: UNIT)
            if crate::exec::env_flag!("PHITEX_AUX_TRACE") {
                eprintln!(
                    "aux-trace: plan page {j}: fired {} flushes {:?} stuck {}",
                    rec.fired.is_some(),
                    rec.flushes,
                    rec.stuck
                );
            }
            // (once each: a fold that read past the penalty may hand the
            // marker to the next page's fold too)
            for &(n, k, cs) in &rec.flushes {
                if !flushed.insert((n, k)) {
                    continue;
                }
                let after = i64::try_from(j).unwrap_or(0) - 1;
                flushplan.push(v.list(vec![v.int(n), v.int(k), v.int(after), cs]));
            }
            for &node in &rec.passed {
                if published.insert(node) {
                    let n = u64::from_ne_bytes(node.to_ne_bytes());
                    let f = v.field(&crate::etex::marks_before_field(n));
                    if self.cx.is_loopback(f) {
                        lb_pub(self.cx, f, marks.value(v, j > 0));
                    }
                }
            }
            if let Some((ms, end)) = &rec.fired {
                plan.push(v.int(i64::try_from(j).unwrap_or(0)));
                let [t, _, b] = marks.0.get(&0).copied().unwrap_or([None; 3]);
                let u = |m: Option<ValueId>| m.unwrap_or(ValueId::UNIT);
                let mut mk = vec![u(t), u(b)];
                // (and the other classes' marks, e-TeX's `\\marks`: from
                // the plan too, never from the step before)
                let classes = marks.classes(v);
                if !classes.is_empty() {
                    mk.push(v.list(classes));
                }
                before.push(v.list(mk));
                marks.fire_values(v, ms);
                n_starts += 1;
                start = end.clone();
            } else {
                if rec.stuck {
                    self.diag(
                        true,
                        "\\end could not empty the page (an output routine that never ships?)",
                    );
                }
                ended = true;
            }
            recs.push(rec);
            j += 1;
        }
        let t2 = t0.elapsed();
        // (the start after the last page folds to the end: no `brk`)
        let pages = n_starts - 1;
        let wp = v.seq_unit(v.list(plan));
        self.contribute("wplan", wp);
        if !flushplan.is_empty() {
            self.contribute("flushplan", v.seq_unit(v.list(flushplan)));
        }
        // the fonts the text loaded, by the page that fired after them,
        // and how many fonts the text loaded in all: the writer numbers
        // the fonts as pdfTeX does (`pdfout::font_tags`)
        // (exact mode only: in relaxed mode the names are free)
        if !crate::pdfrelaxed::on(self)? {
            let mut fp = vec![v.int(self.cint(crate::fields::cell::FONTCOUNT)?)];
            fp.extend(fonts_at.iter().copied().take(pages));
            self.contribute("fplan", v.seq_unit(v.list(fp)));
            // the objects written after page j-1 fired, before page j
            // (the last list: after every page)
            if objs_at.iter().any(|&l| !list_of(v, l).is_empty()) {
                self.contribute("oplan", v.seq_unit(v.list(objs_at.clone())));
            }
        }
        // each page's own material (`pp:items:<j>`, its start rebased to
        // it): a `brk` reads only its page, so an edit elsewhere on the
        // list wakes no other page's `brk`
        let mut fields = prev.map(|p| p.fields).unwrap_or_default();
        for j in fields.len()..pages {
            let f = |k: &str| self.ops.fields.get_at(v, epoch, &format!("pp:{k}:{j}"));
            fields.push([f("items"), f("start"), f("mk"), f("fire")]);
        }
        // where each page's step reads the text's state (`pp:fire:<j>`):
        // after the node of the text's last definition marker before
        // the page fires (UNIT: none, the unit's start)
        let fires = fire_nodes(v, &recs[..pages.min(recs.len())]);
        for (j, rec) in recs.iter().enumerate().take(pages) {
            self.set_f(fields[j][0], rec.items_v, true);
            self.set_f(fields[j][1], rec.start_v, true);
            self.set_f(fields[j][3], fires[j], true);
        }
        self.set("pp:fires", v.list(fires), true);
        for (j, &m) in before.iter().enumerate().take(pages) {
            self.set_f(fields[j][2], m, true);
        }
        // (the items after the last page's start: kept, as the next run
        // takes them from here)
        let last = recs
            .get(pages)
            .map_or(items.len(), |r| r.start.idx)
            .min(items.len());
        self.set("pp:tail", v.list(items[last..].to_vec()), true);
        // the text's assignments in each page's part, before its break
        // (and after the last), in TeX's order, with the flush points
        // among them (`[UNIT, 4, node, k]`): the writer folds them with
        // the routines' `\\global`s (`FinalState::load`)
        let tdefs: Vec<ValueId> = recs
            .iter()
            .map(|r| {
                let mut es = Vec::new();
                for &m in &r.tdefs {
                    match nodes::tag(v, m) {
                        nodes::T_DEFS_AT => es.extend(list_of(v, m).into_iter().skip(2)),
                        nodes::T_FLUSH_AT => es.push(v.list(vec![
                            ValueId::UNIT,
                            v.int(4),
                            v.int(crate::val::int_at(v, m, 1)),
                            v.int(crate::val::int_at(v, m, 2)),
                        ])),
                        _ => {}
                    }
                }
                v.list(es)
            })
            .collect();
        let tdefs = v.list(tdefs);
        self.set("pp:tdefs", tdefs, true);
        self.set("pp:classes", table_v, true);
        self.set("pp:any", v.int(i64::from(any)), true);
        self.set("pp:marker", marker, true);
        // the nodes that asked whether a page shipped before them
        // (`T_PDFQ_AT`): the pages fired before each, and the steps
        // told to say whether they ship (`pdf:shipped:<j>`)
        for &(node, j) in &probes {
            let n = u64::from_ne_bytes(node.to_ne_bytes());
            let f = v.field(&crate::etex::fired_before_field(n));
            if self.cx.is_loopback(f) {
                self.cx.publish(f, v.int(i64::try_from(j).unwrap_or(0)));
            }
        }
        if !probes.is_empty() {
            self.set("pp:probe", v.int(1), true);
        }
        // how many steps the chain has: the writer folds their deltas
        // up to here ([`FinalState`])
        self.set("pp:steps", v.int(i64::try_from(pages).unwrap_or(0)), true);
        for j in 0..pages {
            self.kid(self.ops.brk, vec![v.int(i64::try_from(j).unwrap_or(0))]);
        }
        if tm {
            eprintln!(
                "plan: {} items, {pages} pages ({folds} folded, pre {pre} suf {suf}): list fold {t1:?} (acc {t_acc:?}), pages {:?} (ahead {tahead:?}: {} of {} taken; fx {tfx:?}, folds {tfold:?}), sets {:?}",
                items.len(),
                t2.saturating_sub(t1),
                ahead.iter().filter(|a| a.rec.is_none()).count(),
                ahead.len(),
                t0.elapsed().saturating_sub(t2)
            );
        }
        let mut by_idx: phitex_defuse::FxHashMap<usize, Vec<usize>> =
            phitex_defuse::FxHashMap::default();
        for (k, r) in recs.iter().enumerate() {
            by_idx.entry(r.start.idx).or_default().push(k);
        }
        crate::planinc::keep(
            v,
            crate::planinc::Plan {
                items: folded,
                ins,
                table: table_v,
                marker,
                pages: recs,
                fields,
                by_idx,
                epoch,
                spec: self.cx.is_speculative(),
            },
        );
        Ok(())
    }

    /// `brk:<j>`: page `j`'s fold (§994–§1017) from its start
    /// (`pp:start:<j>`) to its fire (§1005), the page fired as a child
    /// `PageStep`.
    pub(crate) fn brk(&mut self, args: &[ValueId]) -> R<()> {
        let v = self.v;
        let j = usize::try_from(args.first().map_or(0, |&x| int_of(v, x))).unwrap_or(0);
        let Some(start) = self
            .get(&format!("pp:start:{j}"))?
            .and_then(|x| PageStart::of(v, x))
        else {
            return Ok(());
        };
        // (the page's own material, borrowed, not copied)
        let items_v = self.get(&format!("pp:items:{j}"))?.unwrap_or(ValueId::UNIT);
        let classes = self
            .get("pp:classes")?
            .map(|x| list_of(v, x))
            .unwrap_or_default();
        let op = if self.int("pp:any")? != 0 {
            self.ops.pagestep_any
        } else {
            self.ops.pagestep
        };
        let marker0 = self.get("pp:marker")?.unwrap_or(ValueId::UNIT);
        let fx = if j > 0 {
            self.page_effects(j - 1)?
        } else {
            Vec::new()
        };
        let guard = v.get(items_v);
        let items: &[ValueId] = match &*guard {
            Value::List(xs) => xs,
            _ => &[],
        };
        let out = fold_page(v, items, &classes, marker0, &start, fx, None);
        drop(guard);
        if let Some((f, side, _)) = out.fired {
            let data = self.page_data(&f, &side)?;
            let mk = self.get(&format!("pp:mk:{j}"))?.unwrap_or(ValueId::UNIT);
            // whether the exact PDF writer folds the pages (`pdf:fold`):
            // only then does a step define anything (`pdf:state`)
            let fold = self.cint(crate::fields::cell::PDF_FOLD)?;
            let fire = self.get(&format!("pp:fire:{j}"))?.unwrap_or(ValueId::UNIT);
            self.kid(
                op,
                vec![
                    v.int(i64::try_from(j).unwrap_or(0)),
                    data,
                    mk,
                    v.int(fold),
                    fire,
                ],
            );
        }
        Ok(())
    }
    /// What routine `j` left for the fold in the previous round
    /// (`pagefx:<j>`, a loop-back): items to go before the rest.
    fn page_effects(&mut self, j: usize) -> R<Vec<ValueId>> {
        if j >= MAX_PAGES {
            return Ok(Vec::new());
        }
        let v = self.v;
        let f = self.keyed(crate::fields::Fam::Pagefx, &j.to_string());
        if !self.cx.is_loopback(f) {
            return Ok(Vec::new());
        }
        let x = self.cx.use_field(f)?;
        let xs = x.map(|x| list_of(v, x)).unwrap_or_default();
        let e = match x {
            Some(x) if !xs.is_empty() => vec![("pagefx".to_owned(), x)],
            _ => Vec::new(),
        };
        if crate::exec::env_flag!("PHITEX_PGST") {
            eprintln!("pagefx read {j}: {} items", xs.len());
        }
        log_read(&self.ops, "pagefx", j, e);
        Ok(xs)
    }

    /// A fired page as the step's argument: `[box255, inserts, marks,
    /// held, output_penalty]`, the marks as values (§1012–§1021).
    #[allow(clippy::unnecessary_wraps)]
    pub(crate) fn page_data(&mut self, f: &phitex_typeset::page::Fired, side: &Side) -> R<ValueId> {
        let v = self.v;
        let b255 = nodes::encode_box(v, &f.box255.node, side);
        let inserts: Vec<ValueId> = f
            .inserts
            .iter()
            .map(|(n, list)| {
                let l: Vec<ValueId> = list.iter().map(|x| nodes::encode(v, x, side)).collect();
                v.list(vec![v.int(i64::from(*n)), v.list(l)])
            })
            .collect();
        let marks: Vec<ValueId> = f
            .marks
            .iter()
            .map(|&k| side.marks.get(k as usize).copied().unwrap_or(ValueId::UNIT))
            .collect();
        let held: Vec<ValueId> = f
            .held_inserts
            .iter()
            .map(|n| nodes::encode(v, n, side))
            .collect();
        Ok(v.list(vec![
            b255,
            v.list(inserts),
            v.list(marks),
            v.list(held),
            v.int(i64::from(f.output_penalty)),
        ]))
    }

    /// `fire_up` (§1012–§1025) for one page, in its step: the marks and
    /// insertion boxes defined, then the output routine run inline on
    /// this run's nest, or the page shipped (§1023).
    pub(crate) fn page_step(&mut self, args: &[ValueId]) -> R<()> {
        let v = self.v;
        let j = self.page_index();
        // the text's fields as TeX's state holds them when this page
        // fires (§1012): read after the page's fire node, never the
        // text's later definitions (`Run::load_slow`)
        self.fire_at = Some(crate::run::FireAt(fire_node(v, args.get(4).copied())));
        let mut readst = ValueId::UNIT;
        let mut readdef = ValueId::UNIT;
        let mut inherited: Vec<(Field, ValueId)> = Vec::new();
        if j == 0 {
            // `\count0..9` as the first page's routine reads them (the
            // text's at the first flush point; round one: the state)
            let f = v.field("pgread0");
            if let Some(x) = self.cx.use_field(f)?.filter(|&x| x != ValueId::UNIT) {
                readdef = x;
                self.preload_counts(x);
            }
        }
        if j > 0 && j < MAX_PAGES {
            // `\count0..9` as the step before this one left them
            // (`pgdef:<j-1>`, a loop-back: the build's guess in round one)
            let f = v.field(&format!("pgdef:{}", j - 1));
            let x = self.cx.use_field(f)?;
            log_read(&self.ops, "pgdef", j - 1, count_entries(v, x));
            if let Some(x) = x {
                readdef = x;
                self.preload_counts(x);
            }
            // every other field the routines before this page left
            // changed (`pgst:<j-1>`, a loop-back: the chain's cumulative
            // delta; round one's guess none, the state before the chain)
            let f = v.field(&format!("pgst:{}", j - 1));
            let x = self.cx.use_field(f)?;
            readst = x.unwrap_or(ValueId::UNIT);
            inherited = pgst_pairs(v, x);
            log_read(&self.ops, "pgst", j - 1, named(v, &inherited));
        }
        // the chain's state over the text's where the page fires: the
        // routines' `\\global`s while TeX's state holds them, and the
        // text's local bindings of the groups open there (§1025, §279,
        // §283: the writer's fold, `TexOrder`)
        for &(f, x) in &inherited {
            let l = self.loc_of(f);
            self.st.preload(l, Some(x));
        }
        self.st.watch_kinds();
        let d = args.get(1).copied().unwrap_or(ValueId::UNIT);
        if self.fire_page(d, args.get(2).copied())? {
            self.run_all()?;
        }
        if j < MAX_PAGES {
            let mut cs = Vec::new();
            for k in 0..10 {
                cs.push(v.int(i64::from(self.reg_int(0, k)?)));
            }
            let counts = v.list(cs);
            if !self.pagefx_pub {
                log_pub(&self.ops, "pagefx", j, Vec::new());
            }
            // the step's own delta, for the writer's fold (`pgdeltas`)
            let mut own: std::collections::BTreeMap<String, (Field, ValueId)> =
                std::collections::BTreeMap::new();
            // (the count registers it only advanced: the writer folds
            // them by their change, the others by their value)
            let mut advanced = Vec::new();
            for (loc, x, _) in self.st.published(v, false) {
                let f = self.loc_field(loc);
                let name = crate::val::field_name(v, f);
                if name == "pdf:state" || step_input(&name) {
                    continue;
                }
                if name.starts_with("count:") && self.st.advanced_only(loc) {
                    advanced.push(f.0);
                }
                own.insert(name, (f, x));
            }
            let pairs: Vec<ValueId> = own.values().map(|&(f, x)| v.list(vec![f.0, x])).collect();
            let e = v.list(vec![
                v.int(i64::try_from(j).unwrap_or(0)),
                v.list(pairs),
                readst,
                counts,
                readdef,
                v.list(advanced),
            ]);
            self.contribute("pgdeltas", v.seq_unit(e));
        }
        Ok(())
    }

    /// `\count0..9` read from `cs` (ten values) in place of the
    /// engine's.
    /// `\count0..9` as the output routine left them (its `\global`
    /// assignments, §1025): defined globally where they differ from
    /// the text's, so the text after the flush, and the nodes after
    /// this one, read them.
    fn define_counts(&mut self, cs: ValueId) -> R<()> {
        let v = self.v;
        let xs = list_of(v, cs);
        if xs.len() != 10 {
            return Ok(());
        }
        for (k, &c) in xs.iter().enumerate() {
            let k = i32::try_from(k).unwrap_or(0);
            let Some(new) = v.as_int(c).and_then(|n| i32::try_from(n).ok()) else {
                continue;
            };
            if self.reg_int(0, k)? != new {
                let l = self.reg_loc(0, k);
                self.fed = true;
                self.set_loc(l, crate::runstate::Native::Int(new), true);
                self.fed = false;
            }
        }
        Ok(())
    }

    fn preload_counts(&mut self, cs: ValueId) {
        let v = self.v;
        let xs = list_of(v, cs);
        if xs.len() != 10 {
            return;
        }
        for (k, &c) in xs.iter().enumerate() {
            let l = self.reg_loc(0, i32::try_from(k).unwrap_or(0));
            self.st.preload(l, Some(c));
        }
    }

    /// `fire_up` (§1012–§1025) for the fired page `d` ([`Self::page_data`]):
    /// the page shipped by the default routine (false), or the output
    /// routine's group opened and its tokens pushed (true): the caller
    /// runs them.
    pub(crate) fn fire_page(&mut self, d: ValueId, mk: Option<ValueId>) -> R<bool> {
        let v = self.v;
        let data = list_of(v, d);
        let a = |i: usize| data.get(i).copied().unwrap_or(ValueId::UNIT);
        let (b255, inserts, marks, held) =
            (a(0), list_of(v, a(1)), list_of(v, a(2)), list_of(v, a(3)));
        let output_penalty = data.get(4).map_or(0, |&x| int_of(v, x));
        if crate::exec::env_flag!("PHITEX_PGST") {
            eprintln!(
                "fire page {}: outputpenalty {output_penalty}, {} inserts",
                self.page_index(),
                inserts.len()
            );
        }
        // §1012: marks
        // (the marks before the page, from the plan: `pp:mk:<j>`)
        let mkl = mk.map(|x| list_of(v, x));
        let (mut top, bot) = match &mkl {
            Some(xs) if xs.len() >= 2 => {
                let m = |x: ValueId| Some(x).filter(|&x| x != ValueId::UNIT);
                (m(xs[0]), m(xs[1]))
            }
            _ => (
                self.get_loc(crate::fields::cell::MARK_TOP)?
                    .filter(|&x| x != ValueId::UNIT),
                self.get_loc(crate::fields::cell::MARK_BOT)?
                    .filter(|&x| x != ValueId::UNIT),
            ),
        };
        let mut first = None;
        let mut new_bot = bot;
        if bot.is_some() {
            top = bot;
        }
        // (the other classes' marks before the page: the plan's)
        let class_before = mk_classes(v, mkl.as_deref());
        crate::etex::fire_marks_values(self, &marks, class_before.as_deref())?;
        for &m in &marks {
            if crate::etex::is_class_mark(v, Some(m)) {
                continue;
            }
            if first.is_none() {
                first = Some(m);
            }
            new_bot = Some(m);
        }
        if top.is_some() && first.is_none() {
            first = top;
        }
        for (name, m) in [
            ("mark:top", top),
            ("mark:first", first),
            ("mark:bot", new_bot),
        ] {
            self.set(name, m.unwrap_or(ValueId::UNIT), true);
        }
        // §1018–§1021: insertion boxes
        for e in inserts {
            let xs = list_of(v, e);
            let n = xs.first().map_or(0, |&x| int_of(v, x));
            let old = self.box_reg(i32::try_from(n).unwrap_or(0))?;
            let mut s2 = Side::default();
            let mut l: List = match old.and_then(|b| nodes::decode(v, b, &mut s2)) {
                Some(Node::Box(b)) => b.list,
                _ => Vec::new(),
            };
            for &x in &xs.get(1).map(|&l| list_of(v, l)).unwrap_or_default() {
                if let Some(nd) = nodes::decode(v, x, &mut s2) {
                    l.push(nd);
                }
            }
            let p = phitex_typeset::pack::vpack(l, Spec::Additional(0), &PackParams::default());
            let b = nodes::encode_box(v, &p.node, &s2);
            self.set(&format!("box:{n}"), v.list(vec![v.int(0), b]), true);
        }
        self.set("param:outputpenalty", v.int(output_penalty), true);
        let output = self.par(Par::output)?;
        let otoks = toks_of(v, output);
        if otoks.is_empty() {
            // §1023: the default output routine
            self.ship_out(b255)?;
            if !held.is_empty() {
                self.diag(
                    false,
                    "held-over insertions without an output routine (next round)",
                );
            }
            return Ok(false);
        }
        // §1025
        let dead = self.cint(crate::fields::cell::DEADCYCLES)? + 1;
        self.set_cell(crate::fields::cell::DEADCYCLES, v.int(dead), true);
        self.set_cell(
            crate::fields::cell::BOX255,
            v.list(vec![v.int(0), b255]),
            true,
        );
        // the page parameters the routine starts with: one it changes
        // globally reaches the fold for the next page (`output_end`)
        let before = self.page_params_marker()?;
        self.push_list(-1, group::OUTPUT, vec![v.list(held), before]);
        self.normal_paragraph()?;
        let mut toks = otoks;
        if !toks.is_empty() {
            toks.remove(0);
        }
        self.push_toks(toks);
        Ok(true)
    }
}

/// The output group's `}` (§1026): the routine's list and the held-over
/// insertions are what the next page sees first (`pagefx:<j>`, next
/// round); `\box255` must be void.
pub(crate) fn output_end(r: &mut Run<'_, '_>, md: &Mode) -> R<()> {
    let v = r.v;
    let (items, _) = r.list_items(md.lid)?;
    let mut left = md.extra.first().map(|&x| list_of(v, x)).unwrap_or_default();
    r.close_group(md)?;
    // §1026: the routine's global \vsize (\maxdepth, \topskip,
    // \holdinginserts) governs the next page's breaks
    let before = md.extra.get(1).copied();
    let after = r.page_params_marker()?;
    if before.is_some_and(|b| b != after) {
        left.insert(0, after);
    }
    if r.box_reg(255)?.is_some() {
        r.log("error", "Output routine didn't use all of \\box255")?;
    }
    r.set_cell(crate::fields::cell::INSPEN, v.int(0), true);
    left.extend(items);
    if r.pg.is_some() {
        // a text paging inline (`pageinline.rs`): §1026's contributions
        // go back before the rest, and the page builder goes on
        return crate::pageinline::output_done(r, left);
    }
    // (a step that read at a gone position publishes nothing)
    if !left.is_empty() && !r.saw_gone {
        let j = r.page_index();
        if j < MAX_PAGES {
            let x = v.list(left);
            if crate::exec::env_flag!("PHITEX_PGST") {
                let s: String = v.render(x).chars().take(300).collect();
                eprintln!("pagefx publish {j}: {s}");
            }
            let f = r.keyed(crate::fields::Fam::Pagefx, &j.to_string());
            lb_pub(r.cx, f, x);
            log_pub(&r.ops, "pagefx", j, vec![("pagefx".to_owned(), x)]);
            r.pagefx_pub = true;
            // (kept for the client: the next session's round-one guess)
            let jj = i64::try_from(j).unwrap_or(0);
            r.contribute("pagefxs", v.seq_unit(v.list(vec![v.int(jj), x])));
        }
    }
    r.set_cell(crate::fields::cell::DEADCYCLES, v.int(0), true);
    Ok(())
}

impl Run<'_, '_> {
    /// The page parameters in force, as the fold's marker
    /// (`T_PAGE_PARAMS`).
    pub(crate) fn page_params_marker(&mut self) -> R<ValueId> {
        let v = self.v;
        let vsize = self.ipar(Par::vsize)?;
        let maxdepth = self.ipar(Par::maxdepth)?;
        let topskip = self.gpar(Par::topskip)?;
        let holding = self.ipar(Par::holdinginserts)?;
        Ok(v.list(vec![
            v.int(nodes::T_PAGE_PARAMS),
            v.int(i64::from(vsize)),
            v.int(i64::from(maxdepth)),
            crate::val::glue_value(v, &topskip),
            v.int(i64::from(holding)),
        ]))
    }

    /// Which page this step is (its first argument), or `MAX_PAGES`.
    /// A document node forced the page out (§1005, a penalty of -10000
    /// or less on the main vertical list: `\clearpage`): TeX's output
    /// routine runs here, so what the text reads next of the state the
    /// routines change (`\count0`, LaTeX's shipout counter at
    /// `\enddocument`) is the state after the page that breaks here.
    /// The node marks the point on the list (`T_FLUSH_AT`) and reads its
    /// loop-back entry in `pgst:flushes` (the writer publishes it from the
    /// chain: `[counts, pairs]`), its fields read in place of the
    /// engine's from here on; UNIT (the first round) leaves the state as
    /// it is.
    pub(crate) fn flush_point(&mut self) -> R<()> {
        let v = self.v;
        let op = self.cx.op();
        if op == self.ops.pagestep || op == self.ops.pagestep_any || self.pg.is_some() {
            return Ok(());
        }
        let node = self.cx.node().raw();
        let k = self.flushes;
        self.flushes += 1;
        // the text's `\count0..9` as they stand here: what the page the
        // flush opens starts from (the writer's fold replaces its
        // running counts with them: the text's arithmetic since the
        // last flush, on the value fed back there)
        // (the engine's own reads: no probe; the flush is read next)
        self.probe_ok = true;
        let mut before = Vec::with_capacity(10);
        for c in 0..10 {
            before.push(v.int(i64::from(self.reg_int(0, c)?)));
        }
        // (the text's assignments before it go first: the fold reads
        // them in TeX's order)
        self.flush_text_defs();
        let at = v.list(vec![
            v.int(nodes::T_FLUSH_AT),
            v.int(i64::from_ne_bytes(node.to_ne_bytes())),
            v.int(i64::try_from(k).unwrap_or(0)),
            v.list(before),
        ]);
        self.contribute("items", v.seq_unit(v.list(vec![v.int(0), at])));
        // (read on the next count-register read, [`Run::flush_read`])
        self.flush_pending = Some((
            i64::from_ne_bytes(node.to_ne_bytes()),
            i64::try_from(k).unwrap_or(0),
        ));
        Ok(())
    }

    /// The text reads `\\count0..9` where pages may have fired since it
    /// last had them from the chain (the run's start, or an item it put
    /// on the main list since): TeX's text reads them as the output
    /// routines left them (§1025: `\\global` assignments to eqtb, which
    /// the text sees at once). A probe on the main list there (a flush
    /// point's marker with no page forced: `T_FLUSH_AT` with the ten
    /// counts and a 1), read as a flush point is: the state after the
    /// last page fired before it (`pgst:flushes`, the writer's; none in
    /// round one or before the first page). amsmath's `\\savecounters@`
    /// and `\\restorecounters@` in a display write `\\c@page` back
    /// with the value they read.
    pub(crate) fn count_probe(&mut self) -> R<()> {
        self.probe_ok = true;
        let op = self.cx.op();
        if self.fed
            || self.interp.is_some()
            || self.pg.is_some()
            || (op != self.ops.exec && op != self.ops.put)
        {
            return Ok(());
        }
        let v = self.v;
        let node = self.cx.node().raw();
        let k = self.flushes;
        self.flushes += 1;
        let mut before = Vec::with_capacity(11);
        for c in 0..10 {
            before.push(v.int(i64::from(self.reg_int(0, c)?)));
        }
        before.push(v.int(1));
        // (the text's assignments before it go first: the fold reads
        // them in TeX's order)
        self.flush_text_defs();
        let at = v.list(vec![
            v.int(nodes::T_FLUSH_AT),
            v.int(i64::from_ne_bytes(node.to_ne_bytes())),
            v.int(i64::try_from(k).unwrap_or(0)),
            v.list(before),
        ]);
        self.contribute("items", v.seq_unit(v.list(vec![v.int(0), at])));
        self.flush_pending = Some((
            i64::from_ne_bytes(node.to_ne_bytes()),
            i64::try_from(k).unwrap_or(0),
        ));
        self.flush_read_as(false)
    }

    /// The pending flush point withdrawn: nothing of this run read it
    /// (the run ends, or the job does), so no text reads what it would
    /// feed back, and it is no flush point (a marker `[k]`, which drops
    /// it from the fold): plain's `\supereject` before `\end` costs no round.
    pub(crate) fn withdraw_flush(&mut self) {
        let v = self.v;
        if let Some((node, k)) = self.flush_pending.take() {
            // (a marker of its own, the flush's number its payload: the
            // plan keeps one marker per node and number)
            let id = self.flushes;
            self.flushes += 1;
            let at = v.list(vec![
                v.int(nodes::T_FLUSH_AT),
                v.int(node),
                v.int(i64::try_from(id).unwrap_or(0)),
                v.list(vec![v.int(k)]),
            ]);
            self.contribute("items", v.seq_unit(v.list(vec![v.int(0), at])));
        }
    }

    /// The pending flush point's loop-back read: its entry in
    /// [`FLUSHES`] (`[node, k, value]`), its fields read in place of the
    /// engine's from here on; none (round one) leaves the state as it is.
    pub(crate) fn flush_read(&mut self) -> R<()> {
        self.flush_read_as(true)
    }

    /// [`Run::flush_read`]; a count probe's (`define` false) reads the
    /// counts in place of the engine's, defining nothing (the text's
    /// view of the chain's registers, not an assignment of its own).
    fn flush_read_as(&mut self, define: bool) -> R<()> {
        let v = self.v;
        // (pending until read: a stop here rewinds the command, which
        // reads it again)
        let Some((node, k)) = self.flush_pending else {
            return Ok(());
        };
        let all = self.cx.use_field(v.field(FLUSHES))?;
        let read = all.and_then(|all| {
            list_of(v, all).into_iter().find_map(|e| {
                let xs = list_of(v, e);
                (xs.len() == 3 && int_of(v, xs[0]) == node && int_of(v, xs[1]) == k).then(|| xs[2])
            })
        });
        let xs = read
            .filter(|&x| x != ValueId::UNIT)
            .map(|x| list_of(v, x))
            .unwrap_or_default();
        let pairs = pgst_pairs(v, xs.get(1).copied());
        // a field an earlier flush point fed back that the routines no
        // longer leave changed here is the text's own again (multicol's
        // `\\partial@page`, voided by the routine that unboxed it)
        let mut stale = Vec::new();
        if !xs.is_empty() {
            let gs: Vec<Field> = self
                .flush_fields
                .iter()
                .copied()
                .filter(|g| !pairs.iter().any(|p| p.0 == *g))
                .collect();
            for g in gs {
                let l = self.loc_of(g);
                if self.st.is_preloaded(l) {
                    stale.push((l, self.cx.use_field(g)?));
                }
            }
        }
        self.flush_pending = None;
        // (the text has the chain's counts from here on)
        self.probe_ok = true;
        if crate::exec::env_flag!("PHITEX_AUX_TRACE") {
            eprintln!(
                "aux-trace: flush point {node}:{k} reads {}",
                read.map_or("none".to_owned(), |x| v.render(x))
            );
        }
        if xs.is_empty() {
            return Ok(());
        }
        if let Some(&cs) = xs.first() {
            if define {
                // (defined on the text's first count read: a node that
                // reads other fields only defines no count)
                self.flush_cs = Some(cs);
            } else {
                self.preload_counts(cs);
            }
        }
        for (l, x) in stale {
            self.st.fill(l, x);
        }
        for (g, y) in pairs {
            self.flush_fields.insert(g);
            let l = self.loc_of(g);
            self.st.preload_global(l, Some(y));
        }
        Ok(())
    }

    /// The counts a flush point's read fed back, defined now (the text's
    /// first read of a count after it).
    pub(crate) fn flush_counts(&mut self) -> R<()> {
        if let Some(cs) = self.flush_cs.take() {
            self.define_counts(cs)?;
        }
        Ok(())
    }

    pub(crate) fn page_index(&self) -> usize {
        self.cx
            .args()
            .first()
            .and_then(|&x| self.v.as_int(x))
            .and_then(|x| usize::try_from(x).ok())
            .unwrap_or(MAX_PAGES)
    }
}

/// The marks of every class after the pages fired so far: `class ->
/// (top, first, bot)` (§1012–§1014, pdfTeX §1493 ff.).
#[derive(Default)]
struct MarksState(std::collections::BTreeMap<i32, [Option<ValueId>; 3]>);

impl MarksState {
    /// A page fires with the marks `ms` (their values, in order).
    fn fire_values(&mut self, v: &Values, ms: &[ValueId]) {
        let page: Vec<(i32, ValueId)> = ms.iter().map(|&m| crate::etex::mark_class(v, m)).collect();
        for &(c, _) in &page {
            self.0.entry(c).or_default();
        }
        for (&c, st) in &mut self.0 {
            let [top, first, bot] = st;
            if bot.is_some() {
                *top = *bot;
                *first = None;
            }
            for &(mc, text) in &page {
                if mc == c {
                    if first.is_none() {
                        *first = Some(text);
                    }
                    *bot = Some(text);
                }
            }
            if top.is_some() && first.is_none() {
                *first = *top;
            }
        }
    }

    /// The marks of the classes other than 0 as they stand before the
    /// next page fires: `[class, top, first, bot]` each (UNIT: null).
    fn classes(&self, v: &Values) -> Vec<ValueId> {
        let u = |m: Option<ValueId>| m.unwrap_or(ValueId::UNIT);
        self.0
            .iter()
            .filter(|&(&c, _)| c != 0)
            .map(|(&c, [t, f, b])| v.list(vec![v.int(i64::from(c)), u(*t), u(*f), u(*b)]))
            .collect()
    }

    /// The `marks:before:<node>` value: UNIT before the first page.
    fn value(&self, v: &Values, fired: bool) -> ValueId {
        if !fired {
            return ValueId::UNIT;
        }
        let u = |m: Option<ValueId>| m.unwrap_or(ValueId::UNIT);
        v.list(
            self.0
                .iter()
                .map(|(&c, [t, f, b])| v.list(vec![v.int(i64::from(c)), u(*t), u(*f), u(*b)]))
                .collect(),
        )
    }
}

/// Where a page starts in the main list (`pp:start:<j>`): the index of
/// the first item not yet contributed, the material carried over before
/// it (§1017: none when it is the list's own items, which the index then
/// covers), the page parameters in force there (`None`: the first
/// page's), `\end`'s item once met, and the
/// dead cycles `\end` spent (§1054). `"end"`: the list ended before.
#[derive(Clone)]
pub(crate) struct PageStart {
    pub idx: usize,
    pub carried: Vec<ValueId>,
    pub marker: Option<ValueId>,
    pub ending: Option<ValueId>,
    pub deadcycles: u32,
}

impl PageStart {
    pub(crate) const FIRST: PageStart = PageStart {
        idx: 0,
        carried: Vec::new(),
        marker: None,
        ending: None,
        deadcycles: 0,
    };

    pub(crate) fn value(&self, v: &Values) -> ValueId {
        let o = |x: Option<ValueId>| x.unwrap_or(ValueId::UNIT);
        v.list(vec![
            v.int(i64::try_from(self.idx).unwrap_or(0)),
            v.list(self.carried.clone()),
            o(self.marker),
            o(self.ending),
            v.int(i64::from(self.deadcycles)),
        ])
    }

    /// The start a value names; `None` for the end (or no value).
    pub(crate) fn of(v: &Values, x: ValueId) -> Option<PageStart> {
        let Value::List(xs) = &*v.get(x) else {
            return None;
        };
        if xs.len() != 5 {
            return None;
        }
        let some = |y: ValueId| (y != ValueId::UNIT).then_some(y);
        Some(PageStart {
            idx: usize::try_from(int_of(v, xs[0])).unwrap_or(0),
            carried: list_of(v, xs[1]),
            marker: some(xs[2]),
            ending: some(xs[3]),
            deadcycles: u32::try_from(int_of(v, xs[4])).unwrap_or(0),
        })
    }
}

/// The page parameters a `T_PAGE_PARAMS` marker carries.
fn set_params(v: &Values, p: &mut PageParams, x: ValueId) {
    let xs = list_of(v, x);
    if xs.len() < 5 {
        return;
    }
    let n = |i: usize| i32::try_from(xs.get(i).map_or(0, |&y| int_of(v, y))).unwrap_or(0);
    p.vsize = n(1);
    p.max_depth = n(2);
    p.top_skip = crate::val::glue_of(v, xs.get(3).copied());
    p.holding_inserts = n(4);
}

/// What one page's fold found: the fired page with its side tables and
/// the next page's start (`None`: the list ended first), the nodes
/// reading marks it passed, and whether `\end` could not empty the page.
struct PageFold {
    fired: Option<(Box<phitex_typeset::page::Fired>, Side, PageStart)>,
    passed: Vec<i64>,
    /// The flush points met (`T_FLUSH_AT`): `[node, k]`.
    flushes: Vec<(i64, i64, ValueId)>,
    /// The immediate writes' entries met (`T_WRITE_AT`), in order.
    writes: Vec<ValueId>,
    /// The text's definition markers met (`T_DEFS_AT`), in order.
    tdefs: Vec<ValueId>,
    stuck: bool,
    /// The end of the list's items the fold read (its cursor, or one
    /// past it where a kern looked at what follows).
    reach: usize,
}

/// A page as the plan folds it: the end of the items it read, the marks
/// it fired with and where the next page starts (`None`: no fire), the
/// marks classes passed, the immediate writes met, whether `\\end` was
/// stuck.
struct PlanPage {
    reach: usize,
    fired: Option<(Vec<ValueId>, PageStart)>,
    passed: Vec<i64>,
    writes: Vec<ValueId>,
    /// The flush points met (`T_FLUSH_AT`): `[node, k]`.
    flushes: Vec<(i64, i64, ValueId)>,
    tdefs: Vec<ValueId>,
    stuck: bool,
}

/// The plan's record of a page folded from `start`.
fn page_rec(
    v: &Values,
    items: &[ValueId],
    start: &PageStart,
    fx: Vec<ValueId>,
    out: PlanPage,
) -> crate::planinc::PageRec {
    let (a, b) = (start.idx.min(items.len()), out.reach.min(items.len()));
    crate::planinc::PageRec {
        start: start.clone(),
        fx,
        reach: out.reach,
        fired: out.fired,
        passed: out.passed,
        writes: out.writes,
        tdefs: out.tdefs,
        stuck: out.stuck,
        flushes: out.flushes,
        items_v: v.list(items[a..b.max(a)].to_vec()),
        start_v: PageStart {
            idx: 0,
            ..start.clone()
        }
        .value(v),
    }
}

/// The list's items as a prediction reads them: their kinds, and their
/// boxes' dimensions (`None` where the walk decodes the item itself).
#[derive(Default)]
struct Profile {
    tags: Vec<i64>,
    nodes: Vec<Option<Node>>,
}

/// A page folded ahead of the plan's walk: its predicted start and the
/// record (taken by the walk when the start is the real one).
struct Ahead {
    start: PageStart,
    fx: Vec<ValueId>,
    rec: Option<std::sync::Arc<crate::planinc::PageRec>>,
}

/// The pages of `items` folded ahead, in parallel: the starts predicted
/// by one walk of the break arithmetic over the boxes' dimensions alone
/// ([`nodes::decode_shallow`], the routines taken as leaving nothing),
/// then each page's real fold ([`plan_fold`]) from its predicted start
/// on the executor's workers (`exec::par_each`). No page reads another:
/// the plan's walk takes a page only where its start and what the
/// routine before it left are the real ones, and folds the others.
fn ahead_folds(v: &Values, items: &[ValueId], table: ValueId, marker0: ValueId) -> Vec<Ahead> {
    let tp0 = std::time::Instant::now();
    let classes = list_of(v, table);
    let mut starts = Vec::new();
    let mut s = PageStart::FIRST;
    let was = crate::val::NO_KEEP.with(|c| c.replace(true));
    let chunk = 1024;
    let parts = crate::exec::par_map(items.len().div_ceil(chunk), &|k| {
        let xs = &items[k * chunk..((k + 1) * chunk).min(items.len())];
        let tags: Vec<i64> = xs.iter().map(|&x| nodes::tag(v, x)).collect();
        // (an item a marks or a whatsit decodes with is decoded in the
        // walk itself, which keeps them)
        let nodes: Vec<Option<Node>> = xs
            .iter()
            .map(|&x| {
                let mut side = Side::default();
                let n = nodes::decode_shallow(v, x, &mut side);
                (side.marks.is_empty() && side.whats.is_empty())
                    .then_some(n)
                    .flatten()
            })
            .collect();
        (tags, nodes)
    });
    crate::val::NO_KEEP.with(|c| c.set(was));
    let mut prof = Profile::default();
    for (t, n) in parts {
        prof.tags.extend(t);
        prof.nodes.extend(n);
    }
    let was = crate::val::NO_KEEP.with(|c| c.replace(true));
    let tprof = tp0.elapsed();
    while starts.len() < MAX_PAGES {
        let out = fold_page(v, items, &classes, marker0, &s, Vec::new(), Some(&prof));
        starts.push(s);
        match out.fired {
            Some((_, _, end))
                if end.idx > starts[starts.len() - 1].idx || !end.carried.is_empty() =>
            {
                s = end;
            }
            _ => break,
        }
    }
    crate::val::NO_KEEP.with(|c| c.set(was));
    let tw = std::time::Instant::now();
    let tpred = tp0.elapsed();
    let slots: Vec<std::sync::OnceLock<crate::planinc::PageRec>> =
        starts.iter().map(|_| std::sync::OnceLock::new()).collect();
    crate::exec::par_each(starts.len(), &|j| {
        let out = plan_fold(v, items, table, marker0, &starts[j], Vec::new());
        let _ = slots[j].set(page_rec(v, items, &starts[j], Vec::new(), out));
    });
    if crate::exec::env_flag!("PHITEX_PLAN_TIME") {
        eprintln!(
            "pages ahead: {} predicted in {tpred:?} (profile {tprof:?}), folds {:?}",
            starts.len(),
            tw.elapsed()
        );
    }
    starts
        .into_iter()
        .zip(slots)
        .map(|(start, r)| Ahead {
            start,
            fx: Vec::new(),
            rec: r.into_inner().map(std::sync::Arc::new),
        })
        .collect()
}

/// A memo entry: the items the fold read (from the start's index), and
/// the page with the next start's index relative to it, its carried
/// material kept outside the value table (a sweep may free it).
struct PlanMemo {
    items: Vec<ValueId>,
    /// Whether the fold read to the list's end (then a hit needs the
    /// list to end there too).
    to_end: bool,
    fired: Option<(Vec<ValueId>, usize, Vec<crate::api::Tree>, PageStart)>,
    passed: Vec<i64>,
    writes: Vec<ValueId>,
    flushes: Vec<(i64, i64, ValueId)>,
    tdefs: Vec<ValueId>,
    stuck: bool,
}

type PlanCache = std::sync::Mutex<phitex_defuse::FxHashMap<u64, std::sync::Arc<PlanMemo>>>;

/// Plan folds by their inputs: the table, the start (its index aside),
/// what the routine before left, the marker, the first item.
fn plan_cache() -> &'static PlanCache {
    static C: std::sync::OnceLock<PlanCache> = std::sync::OnceLock::new();
    C.get_or_init(PlanCache::default)
}

/// [`fold_page`] for the plan, memoized: an edit folds again only the
/// pages whose material, start or routine effects changed. The fold is
/// a function of its inputs, the items it read compared by value, so a
/// hit is the fold's result. Everything a hit returns is one of the
/// inputs' values (the items, their parts, the start's) except the
/// carried material, which is interned again.
fn plan_fold(
    v: &Values,
    items: &[ValueId],
    table: ValueId,
    marker0: ValueId,
    start: &PageStart,
    fx: Vec<ValueId>,
) -> PlanPage {
    let rebased = PageStart {
        idx: 0,
        ..start.clone()
    }
    .value(v);
    let mut words = vec![
        v.instance(),
        rebased.raw(),
        table.raw(),
        marker0.raw(),
        fx.len() as u64,
    ];
    // (the first items too: pages often begin alike, with glue, and
    // a line's box makes the key the page's)
    let from = start.idx.min(items.len());
    words.extend(
        items[from..(from + 8).min(items.len())]
            .iter()
            .map(|x| x.raw()),
    );
    words.extend(fx.iter().map(|x| x.raw()));
    let key = phitex_defuse::hash_words(11, &words);
    let hit = plan_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .cloned();
    if let Some(m) = hit
        && items[from..].starts_with(&m.items)
        && (!m.to_end || items.len() == from + m.items.len())
    {
        return PlanPage {
            reach: from + m.items.len(),
            fired: m.fired.as_ref().map(|(ms, idx, carried, end)| {
                let mut end = end.clone();
                end.idx = from + idx;
                end.carried = carried.iter().map(|t| crate::api::import(v, t)).collect();
                (ms.clone(), end)
            }),
            passed: m.passed.clone(),
            writes: m.writes.clone(),
            flushes: m.flushes.clone(),
            tdefs: m.tdefs.clone(),
            stuck: m.stuck,
        };
    }
    let classes = list_of(v, table);
    // (the items are decoded once a fold: not kept in this thread's
    // cache, whose copies cost more than the decoding)
    let was = crate::val::NO_KEEP.with(|c| c.replace(true));
    let out = fold_page(v, items, &classes, marker0, start, fx, None);
    crate::val::NO_KEEP.with(|c| c.set(was));
    let fired = out.fired.map(|(f, side, end)| {
        let ms: Vec<ValueId> = f
            .marks
            .iter()
            .filter_map(|&k| side.marks.get(k as usize).copied())
            .collect();
        (ms, end)
    });
    let memo = PlanMemo {
        items: items[from..out.reach.max(from)].to_vec(),
        to_end: out.reach >= items.len(),
        fired: fired.as_ref().map(|(ms, end)| {
            (
                ms.clone(),
                end.idx - from,
                end.carried
                    .iter()
                    .map(|&x| crate::api::export(v, x))
                    .collect(),
                PageStart {
                    idx: 0,
                    carried: Vec::new(),
                    ..end.clone()
                },
            )
        }),
        passed: out.passed.clone(),
        writes: out.writes.clone(),
        flushes: out.flushes.clone(),
        tdefs: out.tdefs.clone(),
        stuck: out.stuck,
    };
    {
        let mut c = plan_cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // (bounded: a long session starts it again)
        if c.len() > 1 << 16 {
            c.clear();
        }
        c.insert(key, std::sync::Arc::new(memo));
    }
    PlanPage {
        reach: out.reach,
        fired,
        passed: out.passed,
        writes: out.writes,
        flushes: out.flushes,
        tdefs: out.tdefs,
        stuck: out.stuck,
    }
}

/// §994–§1017 from `start` to the first fire (§1005): what routine j-1
/// left (`fx`) first (§1026), then the carried material, then the list
/// from the start's index.
#[allow(clippy::too_many_lines)]
fn fold_page(
    v: &Values,
    items: &[ValueId],
    classes: &[ValueId],
    marker0: ValueId,
    start: &PageStart,
    fx: Vec<ValueId>,
    pre: Option<&Profile>,
) -> PageFold {
    let shallow = pre.is_some();
    // an item of the list by its index, or anything else
    let decode = |x: ValueId, at: Option<usize>, side: &mut Side| match (pre, at) {
        (Some(p), Some(i)) => p.nodes[i]
            .clone()
            .or_else(|| nodes::decode_shallow(v, x, side)),
        (Some(_), None) => nodes::decode_shallow(v, x, side),
        (None, _) => nodes::decode(v, x, side),
    };
    let mut origins: Vec<ValueId> = Vec::new();
    // the page parameters in force where the page starts (the start
    // carries them: the list's own marker before it, or a routine's)
    let mut marker = start.marker.unwrap_or(marker0);
    let mut params = PageParams {
        vsize: 0,
        max_depth: 0,
        top_skip: Glue::ZERO,
        holding_inserts: 0,
        saving_vdiscards: false,
    };
    set_params(v, &mut params, marker);
    let mut queue: VecDeque<ValueId> = fx.into();
    queue.extend(start.carried.iter().copied());
    let mut cursor = start.idx;
    let mut reach = cursor;
    let mut pb = PageBuilder::new();
    let mut side = Side::default();
    let mut ending = start.ending;
    let mut deadcycles = start.deadcycles;
    let mut passed = Vec::new();
    let mut writes = Vec::new();
    let mut flushes = Vec::new();
    let mut tdefs = Vec::new();
    let mut class = |n: u16| -> InsertClass {
        for &c in classes {
            let xs = list_of(v, c);
            let g = |i: usize| xs.get(i).map_or(0, |&y| int_of(v, y));
            if g(0) == i64::from(n) {
                let h = g(1);
                return InsertClass {
                    box_height_plus_depth: (h >= 0).then(|| i32::try_from(h).unwrap_or(0)),
                    box_is_hbox: g(2) != 0,
                    count: i32::try_from(g(3)).unwrap_or(0),
                    dimen: i32::try_from(g(4)).unwrap_or(0),
                    skip: crate::val::glue_of(v, xs.get(5).copied()),
                };
            }
        }
        InsertClass {
            box_height_plus_depth: None,
            box_is_hbox: false,
            count: 0,
            dimen: 0,
            skip: Glue::ZERO,
        }
    };
    loop {
        let mut at = None;
        let x = if let Some(x) = queue.pop_front() {
            x
        } else if let Some(&x) = items.get(cursor) {
            at = Some(cursor);
            cursor += 1;
            reach = reach.max(cursor);
            x
        } else {
            if let Some(e) = ending
                && !pb.page.is_empty()
                && deadcycles < 25
            {
                deadcycles += 1;
                let xs = list_of(v, e);
                for &y in xs.iter().skip(1).rev() {
                    queue.push_front(y);
                }
                continue;
            }
            break;
        };
        let t = match (pre, at) {
            (Some(p), Some(i)) => p.tags[i],
            _ => nodes::tag(v, x),
        };
        match t {
            nodes::T_END => {
                ending = Some(x);
                // §1054: an empty page ends the job without a cycle
                if pb.page.is_empty() {
                    continue;
                }
                let xs = list_of(v, x);
                for &y in xs.iter().skip(1).rev() {
                    queue.push_front(y);
                }
                continue;
            }
            nodes::T_ABORT => {
                return PageFold {
                    fired: None,
                    passed,
                    writes,
                    flushes,
                    tdefs,
                    stuck: false,
                    reach,
                };
            }
            nodes::T_PAGE_PARAMS => {
                set_params(v, &mut params, x);
                marker = x;
                continue;
            }
            nodes::T_MARKS_AT => {
                passed.push(crate::val::int_at(v, x, 1));
                continue;
            }
            nodes::T_FLUSH_AT => {
                // (in order with the text's assignments: the writer's
                // fold reads the state there, `FinalState`)
                tdefs.push(x);
                flushes.push((
                    crate::val::int_at(v, x, 1),
                    crate::val::int_at(v, x, 2),
                    list_of(v, x).get(3).copied().unwrap_or(ValueId::UNIT),
                ));
                continue;
            }
            // (a run's local bindings, kept in order with the
            // definition markers: `fire_nodes`)
            nodes::T_DEFS_AT | nodes::T_LOCALS_AT => {
                tdefs.push(x);
                continue;
            }
            nodes::T_WRITE_AT => {
                if let Some(&e) = list_of(v, x).get(1) {
                    writes.push(e);
                }
                continue;
            }
            // (a font load: kept with the writes, the marker itself, for
            // the writer's numbering, `fplan`)
            nodes::T_FONT_AT | nodes::T_PDFW_AT | nodes::T_PDFQ_AT => {
                writes.push(x);
                continue;
            }
            _ => {}
        }
        let Some(p) = decode(x, at, &mut side) else {
            continue;
        };
        if shallow {
            origins.push(x);
        }
        // §1000: only a kern looks at what follows it
        // (the next node TeX's list holds: the fold's markers are none
        // of TeX's nodes, and `\\end`'s items come first where it is)
        let next = if matches!(p, Node::Kern(..)) {
            let mut found = None;
            let mut k = 0;
            loop {
                let (y, yat) = if let Some(&y) = queue.get(k) {
                    (y, None)
                } else if let Some(&y) = items.get(cursor + k - queue.len()) {
                    let i = cursor + k - queue.len();
                    reach = reach.max(i + 1);
                    (y, Some(i))
                } else {
                    break;
                };
                k += 1;
                if crate::build::is_marker(v, y) {
                    continue;
                }
                if nodes::tag(v, y) == nodes::T_END {
                    found = list_of(v, y)
                        .get(1)
                        .and_then(|&z| decode(z, None, &mut Side::default()));
                    break;
                }
                if let Some(n) = decode(y, yat, &mut Side::default()) {
                    found = Some(n);
                    break;
                }
            }
            found
        } else {
            None
        };
        if let Outcome::Fire(f) = pb.contribute(p, next.as_ref(), &params, &mut class) {
            let mut carried: Vec<ValueId> = if shallow {
                // (a prediction, which the real fold checks: a box
                // taken as the item contributed in its place)
                let k = f.contributions.len();
                f.contributions
                    .iter()
                    .enumerate()
                    .map(|(i, n)| match (n, origins.len().checked_sub(k - i)) {
                        (Node::Box(_), Some(o)) => origins[o],
                        _ => nodes::encode(v, n, &side),
                    })
                    .collect()
            } else {
                f.contributions
                    .iter()
                    .map(|n| nodes::encode(v, n, &side))
                    .collect()
            };
            carried.extend(queue.drain(..));
            // the canonical end: the material carried over is the
            // list's own items before the cursor whenever it can be
            let mut idx = cursor;
            if carried.len() <= cursor && items[cursor - carried.len()..cursor] == carried[..] {
                idx = cursor - carried.len();
                carried.clear();
            }
            let end = PageStart {
                idx,
                carried,
                marker: Some(marker),
                ending,
                deadcycles,
            };
            return PageFold {
                fired: Some((f, side, end)),
                passed,
                writes,
                flushes,
                tdefs,
                stuck: false,
                reach,
            };
        }
    }
    PageFold {
        fired: None,
        passed,
        writes,
        flushes,
        tdefs,
        stuck: ending.is_some() && !pb.page.is_empty(),
        reach,
    }
}

/// `pageplan`: the main list folded once, the pages' `brk` nodes.
pub(crate) struct PagePlan(pub Shared);

impl Operation for PagePlan {
    fn name(&self) -> &'static str {
        "pageplan"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn may_define(&self, _: &[ValueId], _: &Values) -> MayDefine {
        MayDefine::Prefixes(vec![PLAN_PREFIX.to_owned()])
    }
    fn may_define_field(&self, _: &[ValueId], v: &Values, f: Field) -> bool {
        client_only(v, f)
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }
    #[allow(clippy::redundant_closure_for_method_calls)]
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        run_node(cx, ops(&self.0), |r| r.page_plan())
    }
}

/// `brk:<j>`: one page's breaks (§994–§1017), the page's step its child.
pub(crate) struct Brk(pub Shared);

impl Operation for Brk {
    fn name(&self) -> &'static str {
        "brk"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn may_define_field(&self, _: &[ValueId], v: &Values, f: Field) -> bool {
        client_only(v, f)
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let args = cx.args();
        run_node(cx, ops(&self.0), |r| r.brk(&args))
    }
}

/// The prefix of the fields the plan defines ([`PagePlan`]).
const PLAN_PREFIX: &str = "pp:";
/// The exact PDF fold's state, the one field a step defines
/// ([`PageStep::may_define`]).
const PDF_STATE: &str = "pdf:state";

/// Whether field `name` is the chain's own: one the plan or a step
/// defines, which a step reads as any node does, not where its page
/// fires ([`Run::fire_at`]).
pub(crate) fn chain_field(name: &str) -> bool {
    name.starts_with(PLAN_PREFIX) || name == PDF_STATE
}

/// One page's step: the output routine run, the page shipped (§1023
/// without one). It defines nothing to the engine: what the routine
/// defines at its base level goes to the chain's delta (`pgdeltas`,
/// folded by the writer into the loop-backs `pgst:<j>`), `\count0..9`
/// to `pgdef:<j>`. The exact PDF writer's page is written on its own
/// (`pdfout::ship`, finished in order by the `Dvi` node); only when the
/// writer folds the pages (`pdf:fold`, a document whose output routine
/// runs a PDF command) is its state (`pdf:state`) a definition.
pub(crate) struct PageStep {
    pub shared: Shared,
    pub any: bool,
}

impl Operation for PageStep {
    fn name(&self) -> &'static str {
        if self.any { "pagestep_any" } else { "pagestep" }
    }
    /// Nothing, but the exact PDF fold's state ([`Run::finish`]) when
    /// the writer folds the pages (the fourth argument).
    fn may_define(&self, args: &[ValueId], v: &Values) -> MayDefine {
        if args.get(3).and_then(|&x| v.as_int(x)).unwrap_or(0) != 0 {
            MayDefine::Prefixes(vec![PDF_STATE.to_owned()])
        } else {
            MayDefine::Nothing
        }
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    /// Never the plan's fields (`pp:`), which the `brk` nodes after a
    /// step read: their walk does not wait on the step for them.
    fn may_define_field(&self, _: &[ValueId], v: &Values, f: Field) -> bool {
        client_only(v, f) && !matches!(&*v.get(f.0), Value::Str(s) if s.starts_with(PLAN_PREFIX))
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let args = cx.args();
        let o = ops(&self.shared);
        let done = o.steps_done.clone();
        let j = args
            .first()
            .and_then(|&x| cx.values().as_int(x))
            .and_then(|x| usize::try_from(x).ok());
        let out = run_node(cx, o, |r| r.page_step(&args))?;
        if let Some(j) = j {
            let mut d = done
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if d.len() <= j {
                d.resize(j + 1, 0);
            }
            d[j] += 1;
        }
        Ok(out)
    }
}

const MAX_DIMEN: i32 = 0o7777777777;

/// `Dvi`: the pages, written (§583–§645). The comment and `\mag` are
/// read at the end.
pub(crate) fn dvi(cx: &mut Ctx<'_>, o: &crate::run::Ops) -> Result<ValueId, Stop> {
    let t0 = std::time::Instant::now();
    let fin = FinalState::load(cx, o);
    let t1 = t0.elapsed();
    let r = fin.and_then(|fin| {
        o.gone.note(cx.node(), fin.gone);
        if fin.gone {
            return Ok(ValueId::UNIT);
        }
        dvi_file(cx, &fin)
    });
    if crate::exec::env_flag!("PHITEX_TEX_TIME") {
        eprintln!(
            "output file (dvi root run): {:?} (final state {t1:?}, file {:?}){}",
            t0.elapsed(),
            t0.elapsed().saturating_sub(t1),
            if r.is_ok() { "" } else { " (stopped)" }
        );
    }
    r
}

fn dvi_file(cx: &mut Ctx<'_>, fin: &FinalState) -> Result<ValueId, Stop> {
    let v = cx.values();
    let pages = cx.acc_range(v.field("pages"), None)?;
    let all = v.seq_to_vec(pages);
    // the format pdfTeX fixed at the first `\shipout` (`fix_pdfoutput`)
    let pdf = all.first().is_some_and(|&p| {
        list_of(v, p)
            .get(3)
            .is_some_and(|&x| crate::val::int_at(v, x, 0) != 0)
    });
    if pdf {
        if crate::pdfrelaxed::finishing(cx, fin)? {
            return crate::pdfrelaxed::finish(cx, fin);
        }
        return crate::pdfout::finish(cx, fin);
    }
    let comment = fin
        .get(cx, v.field("dvicomment"))?
        .map(|x| str_of(v, x))
        .unwrap_or_default();
    let mag = fin
        .get(cx, v.field("param:mag"))?
        .map_or(1000, |x| int_of(v, x));
    let mut w = phitex_dvi::DviWriter::new(i32::try_from(mag).unwrap_or(1000), &comment);
    // each page's bytes as the previous run wrote them, shipped again
    // when they are what this place would get (`DviWriter::ship_kept`)
    let epoch = cx.values_epoch();
    let old = dvi_memo_take(v, epoch);
    let mut kept = phitex_defuse::FxHashMap::default();
    let t0 = std::time::Instant::now();
    let mut again = 0usize;
    // the file as characters 0–255 (a string value): each page's but
    // the pointer to the previous `bop` kept with its bytes
    let mut s = latin1(w.bytes());
    let nokeep = crate::exec::env_flag!("PHITEX_DVI_NOKEEP");
    let mut written = write_apart(
        v,
        &all,
        old.as_ref(),
        i32::try_from(mag).unwrap_or(1000),
        &comment,
        nokeep,
    );
    let mut apart = 0usize;
    for (i, page) in all.into_iter().enumerate() {
        let at = w.location();
        if let Some(p) = written.remove(&i)
            && w.ship_kept(&p.bytes)
        {
            push_page(&mut s, &w.bytes()[at..], &p);
            kept.insert(page, p);
            apart += 1;
            continue;
        }
        if let Some(p) = old.as_ref().and_then(|m| m.pages.get(&page))
            && w.ship_kept(&p.bytes)
        {
            push_page(&mut s, &w.bytes()[at..], p);
            kept.insert(page, std::sync::Arc::clone(p));
            again += 1;
            continue;
        }
        match dvi_page(v, &mut w, page, !nokeep).and_then(kept_page) {
            Some(k) => {
                push_page(&mut s, &w.bytes()[at..], &k);
                kept.insert(page, k);
            }
            None => s.push_str(&latin1(&w.bytes()[at..])),
        }
    }
    let n = kept.len();
    dvi_memo_keep(
        v,
        DviMemo {
            pages: kept,
            epoch,
            spec: cx.is_speculative(),
        },
    );
    let t1 = t0.elapsed();
    let at = w.location();
    let bytes = w.finish();
    let t2 = t0.elapsed();
    // (no file at all without pages, §642)
    match bytes.get(at..) {
        Some(b) => s.push_str(&latin1(b)),
        None => s.clear(),
    }
    let t3 = t0.elapsed();
    let r = v.intern(Value::Str(s.into_boxed_str()));
    if crate::exec::env_flag!("PHITEX_TEX_TIME") {
        eprintln!(
            "dvi file: {n} pages, {again} kept, {apart} written apart, pages {t1:?}, postamble {:?}, string {:?}, value {:?}",
            t2.saturating_sub(t1),
            t3.saturating_sub(t2),
            t0.elapsed().saturating_sub(t3)
        );
    }
    Ok(r)
}

/// The DVI writer's memory between its runs: each page's bytes by the
/// page's value. The values are the previous run's: valid while that
/// run's definitions (or the evaluation it ran in) keep them, as the
/// plan's (`planinc`).
struct DviMemo {
    pages: phitex_defuse::FxHashMap<ValueId, std::sync::Arc<KeptPage>>,
    epoch: u64,
    spec: bool,
}

/// A page's bytes and, as characters, all of them but the pointer to the
/// previous `bop` (bytes 41–44).
struct KeptPage {
    bytes: phitex_dvi::PageBytes,
    head: String,
    tail: String,
}

/// A kept page's characters onto `s`, the pointer from `b` (the page as
/// written).
fn push_page(s: &mut String, b: &[u8], k: &KeptPage) {
    s.push_str(&k.head);
    s.extend(b[41..45].iter().map(|&c| char::from(c)));
    s.push_str(&k.tail);
}

type DviCache = std::sync::Mutex<phitex_defuse::FxHashMap<u64, DviMemo>>;

fn dvi_cache() -> &'static DviCache {
    static C: std::sync::OnceLock<DviCache> = std::sync::OnceLock::new();
    C.get_or_init(DviCache::default)
}

fn dvi_memo_take(v: &Values, epoch: u64) -> Option<DviMemo> {
    let mut c = dvi_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let m = c.remove(&v.instance())?;
    (!m.spec || m.epoch == epoch).then_some(m)
}

fn dvi_memo_keep(v: &Values, m: DviMemo) {
    let mut c = dvi_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // (bounded: one entry a build; a long test run starts again)
    if c.len() > 64 {
        c.clear();
    }
    c.insert(v.instance(), m);
}

/// `bytes` as characters 0–255.
fn latin1(bytes: &[u8]) -> String {
    let mut u = Vec::with_capacity(bytes.len() + bytes.len() / 2);
    for &b in bytes {
        if b < 0x80 {
            u.push(b);
        } else {
            u.extend_from_slice(&[0xc0 | (b >> 6), 0x80 | (b & 0x3f)]);
        }
    }
    String::from_utf8(u).unwrap_or_default()
}

/// One page of the `pages` contribution into `w` (§640), its bytes kept
/// when `keep`.
fn dvi_page(
    v: &Values,
    w: &mut phitex_dvi::DviWriter,
    page: ValueId,
    keep: bool,
) -> Option<phitex_dvi::PageBytes> {
    dvi_page_assuming(v, w, page, keep, &|_| false)
}

/// The pages of `all` the memory `old` lacks, written apart in parallel
/// (`exec::par_each`) by their index.
fn write_apart(
    v: &Values,
    all: &[ValueId],
    old: Option<&DviMemo>,
    mag: i32,
    comment: &str,
    nokeep: bool,
) -> phitex_defuse::FxHashMap<usize, std::sync::Arc<KeptPage>> {
    // the pages the memory lacks, written apart in parallel, each with
    // the fonts its page list names before it taken as defined (§621);
    // `ship_kept` checks each where it lands, else it is written there
    let fresh: Vec<usize> = (0..all.len())
        .filter(|&i| !nokeep && old.is_none_or(|m| !m.pages.contains_key(&all[i])))
        .collect();
    let mut before: Vec<Vec<u32>> = vec![Vec::new(); all.len()];
    if !fresh.is_empty() {
        let mut seen = phitex_defuse::FxHashSet::default();
        for (i, &page) in all.iter().enumerate() {
            let ids = page_font_ids(v, page);
            before[i] = ids.iter().copied().filter(|f| seen.contains(f)).collect();
            seen.extend(ids);
        }
    }
    let slots: Vec<std::sync::OnceLock<Option<std::sync::Arc<KeptPage>>>> =
        fresh.iter().map(|_| std::sync::OnceLock::new()).collect();
    let busy = std::sync::atomic::AtomicU64::new(0);
    let t0 = std::time::Instant::now();
    crate::exec::par_each(fresh.len(), &|k| {
        let tk = std::time::Instant::now();
        let _busy = Busy(&busy, tk);
        let i = fresh[k];
        let mut pw = phitex_dvi::DviWriter::new(mag, comment);
        let pre = &before[i];
        let r =
            dvi_page_assuming(v, &mut pw, all[i], true, &|f| pre.contains(&f)).and_then(kept_page);
        let _ = slots[k].set(r);
    });
    if crate::exec::env_flag!("PHITEX_TEX_TIME") && !fresh.is_empty() {
        eprintln!(
            "dvi pages apart: {} in {:?}, busy {:?}",
            fresh.len(),
            t0.elapsed(),
            std::time::Duration::from_nanos(busy.load(std::sync::atomic::Ordering::Relaxed))
        );
    }
    fresh
        .iter()
        .zip(slots)
        .filter_map(|(&i, s)| s.into_inner().flatten().map(|k| (i, k)))
        .collect()
}

/// Adds the time since its start to a counter when dropped.
struct Busy<'a>(&'a std::sync::atomic::AtomicU64, std::time::Instant);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        let n = u64::try_from(self.1.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.0.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
    }
}

/// A page's kept bytes with their characters.
fn kept_page(p: phitex_dvi::PageBytes) -> Option<std::sync::Arc<KeptPage>> {
    let b = p.bytes();
    if b.len() < 45 {
        return None;
    }
    let (head, tail) = (latin1(&b[..41]), latin1(&b[45..]));
    Some(std::sync::Arc::new(KeptPage {
        head,
        tail,
        bytes: p,
    }))
}

/// The font numbers a page's list names (its `fontdefs`).
fn page_font_ids(v: &Values, page: ValueId) -> Vec<u32> {
    list_of(v, page)
        .get(2)
        .map(|&l| list_of(v, l))
        .unwrap_or_default()
        .into_iter()
        .map(|fx| {
            let ys = list_of(v, fx);
            u32::try_from(ys.first().map_or(0, |&y| int_of(v, y))).unwrap_or(0)
        })
        .collect()
}

/// [`dvi_page`], the fonts `defined` answers taken as defined before
/// the page (no `fnt_def` for them, §621).
fn dvi_page_assuming(
    v: &Values,
    w: &mut phitex_dvi::DviWriter,
    page: ValueId,
    keep: bool,
    defined: &(dyn Fn(u32) -> bool + Sync),
) -> Option<phitex_dvi::PageBytes> {
    let xs = list_of(v, page);
    let mut side = Side::default();
    // (a page is decoded once: its nodes are not kept in this thread's
    // cache, whose copies cost more than the decoding)
    let was = crate::val::NO_KEEP.with(|c| c.replace(true));
    let bx = xs.first().and_then(|&b| nodes::decode(v, b, &mut side));
    crate::val::NO_KEEP.with(|c| c.set(was));
    let Some(Node::Box(bx)) = bx else {
        return None;
    };
    let mut counts = [0i32; 10];
    for (k, c) in xs
        .get(1)
        .map(|&l| list_of(v, l))
        .unwrap_or_default()
        .iter()
        .enumerate()
        .take(10)
    {
        counts[k] = i32::try_from(int_of(v, *c)).unwrap_or(0);
    }
    let mut defs = phitex_defuse::FxHashMap::default();
    for fx in xs.get(2).map(|&l| list_of(v, l)).unwrap_or_default() {
        let ys = list_of(v, fx);
        let id = u32::try_from(ys.first().map_or(0, |&y| int_of(v, y))).unwrap_or(0);
        let name = ys.get(1).map(|&y| str_of(v, y)).unwrap_or_default();
        let s = i32::try_from(ys.get(3).map_or(0, |&y| int_of(v, y))).unwrap_or(0);
        if let Some(&file) = ys.get(2)
            && let Some(l) = crate::fonts::load(v, file, s)
        {
            defs.insert(id, crate::fonts::font_def(&l, &name));
        }
    }
    for (&f, d) in &defs {
        if defined(f) {
            w.assume_defined(f, d.clone());
        }
    }
    let fonts = |f: u32| {
        defs.get(&f).cloned().unwrap_or(phitex_dvi::FontDef {
            checksum: [0; 4],
            size: 0,
            design_size: 0,
            area: Vec::new(),
            name: Vec::new(),
            widths: Vec::new(),
        })
    };
    let whats = side.whats.clone();
    let mut wh = |h: u32, _leaders: bool| -> Option<Vec<u8>> {
        let w = *whats.get(h as usize)?;
        let ys = list_of(v, w);
        if ys.first().map(|&k| str_of(v, k)).as_deref() == Some("special") {
            Some(crate::fonts::bytes_of(
                &ys.get(1).map(|&s| str_of(v, s)).unwrap_or_default(),
            ))
        } else {
            None
        }
    };
    if keep {
        return w.ship_out_kept(&bx, counts, &fonts, &mut wh).ok();
    }
    let _ = w.ship_out(&bx, counts, &fonts, &mut wh);
    None
}

/// What a DVI page shows: the hash of its `bop`..`eop` bytes written
/// alone, the back pointer left out (it names the previous page's
/// place).
fn dvi_shown(v: &Values, page: ValueId) -> u64 {
    let mut w = phitex_dvi::DviWriter::new(1000, "");
    let at = w.location();
    dvi_page(v, &mut w, page, false);
    let b = &w.bytes()[at..];
    if b.len() < 45 {
        return phitex_defuse::hash_bytes(0, b);
    }
    phitex_defuse::hash_bytes(phitex_defuse::hash_bytes(0, &b[..41]), &b[45..])
}

/// A loop-back's value as named entries, for the refutation report
/// ([`LbLog`]): `pgdef:<j>`'s `\count0..9`.
fn count_entries(v: &Values, x: Option<ValueId>) -> Vec<(String, ValueId)> {
    x.map(|x| list_of(v, x))
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(k, c)| (format!("count:{k}"), c))
        .collect()
}

/// The other classes' marks before a page, from its `pp:mk:<j>`
/// (`[top, bot, classes?]`): `None` when the plan gave no marks.
fn mk_classes(v: &Values, mk: Option<&[ValueId]>) -> Option<Vec<ValueId>> {
    let xs = mk.filter(|xs| xs.len() >= 2)?;
    Some(xs.get(2).map(|&c| list_of(v, c)).unwrap_or_default())
}

/// Each page's fire node (`pp:fire:<j>`): the node of the text's last
/// `T_DEFS_AT` marker met up to the page's fire, as an integer (UNIT
/// while none: the unit's start). A page step reads the text there:
/// the state the text left when TeX fired the page (§1012), its later
/// definitions unseen.
fn fire_nodes(v: &Values, recs: &[std::sync::Arc<crate::planinc::PageRec>]) -> Vec<ValueId> {
    let mut cur = ValueId::UNIT;
    recs.iter()
        .map(|r| {
            for &m in &r.tdefs {
                if nodes::tag(v, m) == nodes::T_DEFS_AT {
                    cur = v.int(crate::val::int_at(v, m, 1));
                }
            }
            cur
        })
        .collect()
}

/// The node a fire value names (`pp:fire:<j>`): `None` for the unit's
/// start.
pub(crate) fn fire_node(v: &Values, x: Option<ValueId>) -> Option<phitex_defuse::NodeId> {
    let x = x.filter(|&x| x != ValueId::UNIT)?;
    // (`[node, bindings]`: the node first)
    let x = if v.as_int(x).is_some() {
        x
    } else {
        list_of(v, x)
            .first()
            .copied()
            .filter(|&y| y != ValueId::UNIT)?
    };
    v.as_int(x)
        .map(|n| phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(n.to_ne_bytes())))
}

/// `pgst:<j>`'s pairs `[field, value]` (UNIT: none).
fn pgst_pairs(v: &Values, x: Option<ValueId>) -> Vec<(Field, ValueId)> {
    x.map(|x| list_of(v, x))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| {
            let xs = list_of(v, e);
            match xs[..] {
                [f, x] => Some((Field(f), x)),
                _ => None,
            }
        })
        .collect()
}

fn named(v: &Values, pairs: &[(Field, ValueId)]) -> Vec<(String, ValueId)> {
    pairs
        .iter()
        .map(|&(f, x)| (crate::val::field_name(v, f), x))
        .collect()
}

/// What a step defines that the next step defines again from its own
/// argument before anything reads it (§1012 the marks, §1013
/// `\outputpenalty`, §1017 `\box255`), and `\count0..9`
/// (`pgdef:<j>`): not in the chain's delta.
fn step_input(name: &str) -> bool {
    matches!(
        name,
        "mark:top" | "mark:first" | "mark:bot" | "box:255" | "param:outputpenalty"
    ) || ["marks:top:", "marks:first:", "marks:bot:"]
        .iter()
        .any(|p| name.starts_with(p))
        || name
            .strip_prefix("count:")
            .is_some_and(|k| k.len() == 1 && k.as_bytes()[0].is_ascii_digit())
}

/// Whether `x` is a field's value as TeX starts it (§222–§240: zero,
/// void, empty), which an undefined field reads as.
fn initial_like(v: &Values, name: &str, x: Option<ValueId>) -> bool {
    let Some(x) = x else { return true };
    match &*v.get(x) {
        Value::Unit | Value::Nil | Value::Int(0) => true,
        Value::List(xs) => {
            xs.is_empty() || (name.starts_with("box:") && xs.len() == 2 && xs[1] == ValueId::UNIT)
        }
        _ => false,
    }
}

/// Whether a step's value `x` of `name` equals the value before the
/// page chain (`pre`).
fn same_as_before(v: &Values, name: &str, pre: Option<ValueId>, x: ValueId) -> bool {
    pre == Some(x) || (initial_like(v, name, pre) && initial_like(v, name, Some(x)))
}

/// [`Run::finish`] of a page step: nothing reaches the engine (the
/// step's delta went to `pgdeltas` in [`Run::page_step`]) but the exact
/// PDF writer's fold (`pdf:state`).
pub(crate) fn step_defs(r: &mut Run<'_, '_>, defs: Vec<(Field, ValueId, Locality)>) {
    let v = r.v;
    for (f, x, l) in defs {
        crate::exec::widths::def(f);
        if crate::val::field_name(v, f) == "pdf:state" {
            r.cx.define_with(f, x, l);
        }
    }
}

/// The TeX state after the page chain, for the writer (§4 of the
/// chain's design): the fields the steps' deltas leave changed, read in
/// place of their reaching definitions (the state before the chain).
/// Loading it folds the steps' deltas (`pgdeltas`) in page order and
/// publishes each page's cumulative delta as `pgst:<j>`, the next
/// round's input of step `j+1`: a change made by one routine reaches
/// every later page in one round.
pub(crate) struct FinalState {
    /// A fire position was gone while it loaded: what it folded is
    /// stale and nothing was published from there on (the writer
    /// publishes nothing either).
    gone: bool,
    map: phitex_defuse::FxHashMap<Field, ValueId>,
}

impl FinalState {
    #[allow(clippy::too_many_lines)]
    pub(crate) fn load(cx: &mut Ctx<'_>, o: &crate::run::Ops) -> Result<FinalState, Stop> {
        let v = cx.values();
        let all = cx.acc_range(v.field("pgdeltas"), None)?;
        let steps = cx
            .use_field(v.field("pp:steps"))?
            .map_or(0, |x| usize::try_from(int_of(v, x)).unwrap_or(0))
            .min(MAX_PAGES);
        // each step's entry: [j, own pairs, pgst read, counts, pgdef
        // read, the count registers only advanced] (`Run::page_step`)
        let entries = Self::entries(v, all);
        // the text's assignments in page k's part, before fire k (k =
        // steps: after the last), in TeX's order, the flush points among
        // them (`Run::log_def`, `PagePlan`)
        let parts: Vec<Vec<ValueId>> = cx
            .use_field(v.field("pp:tdefs"))?
            .map(|x| list_of(v, x).into_iter().map(|p| list_of(v, p)).collect())
            .unwrap_or_default();
        // where each step read the text (`pp:fires`, the plan's): the
        // text's state when page `j` fired, for the fields its routine's
        // delta does not carry
        let fire_vals: Vec<ValueId> = cx
            .use_field(v.field("pp:fires"))?
            .map(|x| list_of(v, x))
            .unwrap_or_default();
        let fires: Vec<Option<phitex_defuse::NodeId>> =
            fire_vals.iter().map(|&x| fire_node(v, Some(x))).collect();
        // the text's `f` as step `k` reads it (after the last step: the
        // text's final state, the writer's)
        // (a gone fire position: `pp:fires` is stale, and re-reading it
        // in this run gives the same list; from there on nothing is
        // published, and the plan's re-run re-binds the writer)
        let gone = std::cell::Cell::new(false);
        let text_at = |cx: &mut Ctx<'_>, f: Field, k: usize| -> Result<Option<ValueId>, Stop> {
            match fires.get(k) {
                Some(&at) if k < steps => Ok(match cx.use_field_at(f, at)? {
                    phitex_defuse::FieldAt::Def(x, _) => Some(x),
                    phitex_defuse::FieldAt::Unset => None,
                    phitex_defuse::FieldAt::Gone => {
                        gone.set(true);
                        None
                    }
                }),
                _ => cx.use_field(f),
            }
        };
        let mut refuted: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut cur = ValueId::UNIT;
        // the flush points (`flushplan`: `[node, k, page, counts]`): a
        // forced page's (`\\count0..9` as the text had them there, the
        // chain's start), a count probe's (`[counts…, 1]`), or one
        // nothing read (`[k]`, withdrawn: `\\bye`'s)
        let fp = cx.acc_range(v.field("flushplan"), None)?;
        let marks: Vec<(i64, i64, i64, ValueId)> = v
            .seq_to_vec(fp)
            .iter()
            .flat_map(|&e| list_of(v, e))
            .filter_map(|e| {
                let xs = list_of(v, e);
                let [n, k, p, cs] = xs.as_slice() else {
                    return None;
                };
                Some((int_of(v, *n), int_of(v, *k), int_of(v, *p), *cs))
            })
            .collect();
        let ints =
            |x: ValueId| -> Vec<i64> { list_of(v, x).iter().map(|&y| int_of(v, y)).collect() };
        let (all_flushes, rest): (Vec<_>, Vec<_>) =
            marks.into_iter().partition(|m| list_of(v, m.3).len() == 10);
        let (probes, rest): (Vec<_>, Vec<_>) =
            rest.into_iter().partition(|m| list_of(v, m.3).len() == 11);
        let unread: Vec<_> = rest
            .into_iter()
            .filter(|m| list_of(v, m.3).len() == 1)
            .collect();
        let flushes: Vec<_> = all_flushes
            .iter()
            .filter(|f| {
                !unread
                    .iter()
                    .any(|u| u.0 == f.0 && crate::val::int_at(v, u.3, 0) == f.1)
            })
            .copied()
            .collect();
        let fires_at = |page: usize| -> Option<Vec<i64>> {
            let page = i64::try_from(page).ok()?;
            all_flushes
                .iter()
                .filter(|f| f.2 == page)
                .map(|f| ints(f.3))
                .find(|c| c.len() == 10)
        };
        // `\count0..9` before the chain: what the first page's routine
        // reads (`pgread0`): the text's at the first flush point, the
        // pages before it read them as the text left them there; with
        // none, the text's where the first page fires
        let start = fires_at(0).or_else(|| {
            all_flushes
                .iter()
                .min_by_key(|f| f.2)
                .map(|f| ints(f.3))
                .filter(|c| c.len() == 10)
        });
        let counts = match start {
            Some(c) => c,
            None => Self::counts(cx, &|cx: &mut Ctx<'_>, f| text_at(cx, f, 0))?,
        };
        let count_fields: Vec<Field> = (0..10).map(|k| v.field(&format!("count:{k}"))).collect();
        let mut tex = TexOrder::new(counts);
        // the text's assignments before the first page fires
        let part0_counts = tex.replay(
            v,
            parts.first().map_or(&[][..], Vec::as_slice),
            &count_fields,
            false,
        );
        let pgread0 = v.field("pgread0");
        if (!flushes.is_empty() || part0_counts) && cx.is_loopback(pgread0) && !gone.get() {
            lb_pub(cx, pgread0, tex.counts_value(v));
        }
        for j in 0..steps {
            let Some(xs) = entries.get(&j) else {
                tex.replay(
                    v,
                    parts.get(j + 1).map_or(&[][..], Vec::as_slice),
                    &count_fields,
                    true,
                );
                continue;
            };
            let g = |i: usize| xs.get(i).copied().unwrap_or(ValueId::UNIT);
            // what step j read against what the steps before it left
            // (step 0 reads the state before the chain itself)
            let read_counts = if g(4) != ValueId::UNIT {
                ints(g(4))
            } else if j == 0 {
                Self::counts(cx, &|cx: &mut Ctx<'_>, f| text_at(cx, f, 0))?
            } else {
                Vec::new()
            };
            let read_pairs = pgst_pairs(v, Some(g(2)));
            if j > 0 {
                if g(2) != cur {
                    let read = named(v, &read_pairs);
                    let want = tex.named(v);
                    for n in differing(&read, &want) {
                        if crate::exec::env_flag!("PHITEX_PGST") {
                            let r = |e: &[(String, ValueId)]| {
                                e.iter().find(|p| p.0 == n).map(|p| v.render(p.1))
                            };
                            eprintln!(
                                "pgst refuted at {j}: {n} read {:?} want {:?}",
                                r(&read),
                                r(&want)
                            );
                        }
                        refuted.insert(n);
                    }
                }
                for (k, (a, b)) in read_counts.iter().zip(&tex.counts).enumerate() {
                    if a != b {
                        refuted.insert(format!("count:{k}"));
                    }
                }
            }
            // `\\count0..9` by the step's change, not its result: the page
            // number the routine advances (`\\advancepageno`, or not on a
            // dead cycle) lands right on every page from one round's runs;
            // a routine's change is a `\\global` one (§1025), which hides
            // the text's saves of it (§283)
            let out_counts = ints(g(3));
            if out_counts.len() == 10 && read_counts.len() == 10 {
                for k in 0..10 {
                    let d = out_counts[k] - read_counts[k];
                    if d != 0 {
                        tex.counts[k] += d;
                        tex.hide_saves(count_fields[k]);
                    }
                }
            }
            if crate::exec::env_flag!("PHITEX_PGST") {
                for (f, x) in pgst_pairs(v, Some(g(1))) {
                    let r: String = v.render(x).chars().take(160).collect();
                    eprintln!("pgst step {j} own: {} = {r}", crate::val::field_name(v, f));
                }
            }
            let advanced: Vec<ValueId> = list_of(v, g(5));
            for (f, x) in pgst_pairs(v, Some(g(1))) {
                let name = crate::val::field_name(v, f);
                // (the text's value where the next step reads it)
                let pre = text_at(cx, f, j + 1)?;
                // a count register the step only advanced, by its change,
                // as `\\count0..9` (a routine's counter of pages or
                // shipouts); one it set, by its value (`\\@topnum`
                // set at each column's start: by its change, a stale
                // read would feed back its error)
                let x = if name.starts_with("count:") && advanced.contains(&f.0) {
                    let as_int = |y: Option<ValueId>| y.map_or(0, |y| int_of(v, y));
                    let had = as_int(read_pairs.iter().find(|p| p.0 == f).map(|p| p.1).or(pre));
                    let now = as_int(tex.held(f).or(pre));
                    v.int(now + int_of(v, x) - had)
                } else {
                    x
                };
                // the routine's `\\global` (§1025): TeX's state holds it
                // until the text assigns the field again; equal to the
                // text's value where the next page fires (and no binding
                // of the text's group there hides that), nothing to carry
                let local = tex.in_frame(f);
                tex.global(
                    f,
                    name,
                    x,
                    same_as_before(v, &crate::val::field_name(v, f), pre, x) && !local,
                );
            }
            // the text's assignments before the next page fires, in TeX's
            // order with this page's routine (§277–§283), the flush
            // points among them reading the state there
            tex.replay(
                v,
                parts.get(j + 1).map_or(&[][..], Vec::as_slice),
                &count_fields,
                true,
            );
            cur = tex.pairs(v);
            log_pub(o, "pgst", j, tex.named(v));
            let jj = v.int(i64::try_from(j).unwrap_or(0));
            if cur != ValueId::UNIT && !gone.get() {
                lb_pub(cx, v.field(&format!("pgst:{j}")), cur);
                // (kept for the client: the next session's guess)
                cx.contribute(
                    v.field("pgsts"),
                    v.seq_unit(v.list(vec![v.int(0), jj, cur])),
                );
            }
            let cs = tex.counts_value(v);
            log_pub(o, "pgdef", j, count_entries(v, Some(cs)));
            if !gone.get() {
                lb_pub(cx, v.field(&format!("pgdef:{j}")), cs);
                cx.contribute(v.field("pgsts"), v.seq_unit(v.list(vec![v.int(1), jj, cs])));
            }
        }
        Self::log_refuted(o, refuted);
        // the flush points' loop-backs: the state where each stands (UNIT
        // before the first page)
        let mut out = Vec::with_capacity(flushes.len() + probes.len());
        for &(n, k, _, _) in flushes.iter().chain(&probes) {
            let x = tex.at.get(&(n, k)).copied().unwrap_or(ValueId::UNIT);
            if crate::exec::env_flag!("PHITEX_AUX_TRACE") {
                eprintln!("aux-trace: flush point {n}:{k}: {}", v.render(x));
            }
            out.push(v.list(vec![v.int(n), v.int(k), x]));
        }
        // (none: nothing published, as `pgst:<j>` with no change: a
        // document without flush points takes one round)
        let f = v.field(FLUSHES);
        if !out.is_empty() && cx.is_loopback(f) && !gone.get() {
            lb_pub(cx, f, v.list(out));
        }
        Ok(FinalState {
            gone: gone.get(),
            map: tex.held,
        })
    }

    /// Each step's entry in the chain's accumulation, by page.
    fn entries(v: &Values, all: ValueId) -> std::collections::BTreeMap<usize, Vec<ValueId>> {
        let mut entries = std::collections::BTreeMap::new();
        for e in v.seq_to_vec(all) {
            let xs = list_of(v, e);
            let Some(&j) = xs.first() else { continue };
            entries.insert(usize::try_from(int_of(v, j)).unwrap_or(0), xs);
        }
        entries
    }

    /// `\\count0..9` as `read` gives them (the text's, where the first
    /// page fires).
    #[allow(clippy::type_complexity)]
    fn counts(
        cx: &mut Ctx<'_>,
        read: &dyn Fn(&mut Ctx<'_>, Field) -> Result<Option<ValueId>, Stop>,
    ) -> Result<Vec<i64>, Stop> {
        let v = cx.values();
        let mut counts: Vec<i64> = Vec::with_capacity(10);
        for k in 0..10 {
            counts.push(read(cx, v.field(&format!("count:{k}")))?.map_or(0, |x| int_of(v, x)));
        }
        Ok(counts)
    }

    /// The loop-back log's line for a refuted round.
    fn log_refuted(o: &crate::run::Ops, refuted: std::collections::BTreeSet<String>) {
        if !refuted.is_empty() {
            let names: Vec<String> = refuted.into_iter().collect();
            o.lblog
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .rounds
                .insert(format!("refuted round: {}", names.join(" ")));
        }
    }

    /// Field `f` after the chain.
    pub(crate) fn get(&self, cx: &mut Ctx<'_>, f: Field) -> Result<Option<ValueId>, Stop> {
        if let Some(&x) = self.map.get(&f) {
            return Ok(Some(x));
        }
        cx.use_field(f)
    }
}

/// The page chain's loop-backs as the steps read and published them in
/// one evaluation, by (kind, page): the guesses refuted
/// ([`crate::Stats::refuted_guesses`]).
#[derive(Default)]
pub(crate) struct LbLog {
    reads: std::collections::BTreeMap<LbKey, Vec<Entries>>,
    pubs: std::collections::BTreeMap<LbKey, Entries>,
    /// The writer's refuted rounds, with the fields.
    rounds: std::collections::BTreeSet<String>,
}

/// The names whose values differ between `a` and `b`.
fn differing(a: &[(String, ValueId)], b: &[(String, ValueId)]) -> Vec<String> {
    let mut out = std::collections::BTreeSet::new();
    for (n, x) in a {
        if !b.iter().any(|(m, y)| m == n && y == x) {
            out.insert(n.clone());
        }
    }
    for (n, x) in b {
        if !a.iter().any(|(m, y)| m == n && y == x) {
            out.insert(n.clone());
        }
    }
    out.into_iter().collect()
}

/// A loop-back of the page chain: its kind and page.
type LbKey = (&'static str, usize);
/// A loop-back's value as named entries.
type Entries = Vec<(String, ValueId)>;

fn log_read(o: &crate::run::Ops, kind: &'static str, j: usize, e: Vec<(String, ValueId)>) {
    let mut l = o
        .lblog
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let rs = l.reads.entry((kind, j)).or_default();
    if !rs.contains(&e) {
        rs.push(e);
    }
}

fn log_pub(o: &crate::run::Ops, kind: &'static str, j: usize, e: Vec<(String, ValueId)>) {
    let mut l = o
        .lblog
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    l.pubs.insert((kind, j), e);
}

impl LbLog {
    /// Every guess read that the value published refuted: `refuted:
    /// <fields> at page <n> (<loop-back>)`, the fields whose values
    /// differ.
    pub(crate) fn refuted(&self) -> Vec<String> {
        let mut out: Vec<String> = self.rounds.iter().cloned().collect();
        for (&(kind, j), reads) in &self.reads {
            let Some(p) = self.pubs.get(&(kind, j)) else {
                continue;
            };
            for r in reads {
                if r == p {
                    continue;
                }
                let mut names = std::collections::BTreeSet::new();
                for (n, x) in r {
                    if !p.iter().any(|(m, y)| m == n && y == x) {
                        names.insert(n.as_str());
                    }
                }
                for (n, x) in p {
                    if !r.iter().any(|(m, y)| m == n && y == x) {
                        names.insert(n.as_str());
                    }
                }
                let names: Vec<&str> = names.into_iter().collect();
                out.push(format!(
                    "refuted: {} at page {} ({kind}:{j})",
                    names.join(" "),
                    j + 2
                ));
            }
        }
        out
    }
}

/// The flush points' loop-back: `[node, k, [counts, pairs]]` each, the
/// state after the page each one opens (`FinalState::load`).
pub(crate) const FLUSHES: &str = "pgst:flushes";

/// Publishes loop-back `f`'s value for the next round; `PHITEX_LOOP_TRACE`
/// prints each publication (which keys keep changing round to round).
pub(crate) fn lb_pub(cx: &mut Ctx<'_>, f: Field, x: ValueId) {
    if crate::exec::env_flag!("PHITEX_LOOP_TRACE") {
        let v = cx.values();
        let mut shown = v.render(x);
        if shown.len() > 160 {
            let mut i = 160;
            while !shown.is_char_boundary(i) {
                i -= 1;
            }
            shown.truncate(i);
        }
        eprintln!(
            "lb-pub {} {:x} {shown}",
            crate::val::field_name(v, f),
            x.raw()
        );
    }
    cx.publish(f, x);
}

/// What a group saved of a field (§283): the routine's `\\global` the
/// state held then (`None`: the text's own), and a count register's value.
#[derive(Clone, Copy)]
struct Save {
    held: Option<ValueId>,
    count: Option<i64>,
    /// The text's value the group hid (`None`: one before the log, the
    /// engine's).
    text: Option<ValueId>,
}

/// TeX's assignment order over the page chain (§277–§283), the writer's
/// one fold of it: the text's assignments (`T_DEFS_AT` events,
/// [`Run::log_def`]) where the text made them and each routine's
/// `\\global`s at its fire. A field holds the younger assignment: a
/// routine's `\\global` until the text assigns the field again (then the
/// text's own, which a page step reads where its page fires), and a
/// group's end restores what the group saved unless a `\\global` hid it
/// since. The same rule for every kind of field; `\\count0..9` kept as
/// values (the routines' by their change, [`FinalState::load`]).
struct TexOrder {
    held: phitex_defuse::FxHashMap<Field, ValueId>,
    names: phitex_defuse::FxHashMap<Field, String>,
    /// The text's value of each field it assigned (its latest, or what a
    /// group's end restored).
    text: phitex_defuse::FxHashMap<Field, ValueId>,
    frames: std::collections::BTreeMap<i64, phitex_defuse::FxHashMap<Field, Save>>,
    counts: Vec<i64>,
    /// The state at each flush point (`[counts, pairs]`), by node and number.
    at: phitex_defuse::FxHashMap<(i64, i64), ValueId>,
}

impl TexOrder {
    fn new(counts: Vec<i64>) -> TexOrder {
        TexOrder {
            held: phitex_defuse::FxHashMap::default(),
            names: phitex_defuse::FxHashMap::default(),
            text: phitex_defuse::FxHashMap::default(),
            frames: std::collections::BTreeMap::new(),
            counts,
            at: phitex_defuse::FxHashMap::default(),
        }
    }

    /// Whether a group open here saved `f` (the text's local binding).
    fn in_frame(&self, f: Field) -> bool {
        self.frames.values().any(|fr| fr.contains_key(&f))
    }

    fn held(&self, f: Field) -> Option<ValueId> {
        self.held.get(&f).copied()
    }

    /// A `\\global` assignment of `f`: the saves of it below are hidden
    /// (§283: its level is one now, which `unsave` retains).
    fn hide_saves(&mut self, f: Field) {
        for fr in self.frames.values_mut() {
            fr.remove(&f);
        }
    }

    /// A routine's `\\global` of `f` (`drop`: equal to the text's, held
    /// as the text's).
    fn global(&mut self, f: Field, name: String, x: ValueId, drop: bool) {
        self.hide_saves(f);
        if drop {
            self.held.remove(&f);
        } else {
            self.held.insert(f, x);
            self.names.insert(f, name);
        }
    }

    /// What a page firing here reads in place of the text's fields
    /// where it fires: the routines' values held, and the text's local
    /// bindings of the groups open here (§1025: eqtb as it stands; a
    /// group of the text's open at the fire is no scope the step is in),
    /// by name.
    fn sorted(&self, v: &Values) -> Vec<(String, Field, ValueId)> {
        self.sorted_as(v, true)
    }

    fn sorted_as(&self, v: &Values, locals: bool) -> Vec<(String, Field, ValueId)> {
        let mut xs: Vec<(String, Field, ValueId)> = self
            .held
            .iter()
            .map(|(&f, &x)| (self.names.get(&f).cloned().unwrap_or_default(), f, x))
            .collect();
        let mut seen = phitex_defuse::FxHashSet::default();
        for fr in self.frames.values().filter(|_| locals) {
            for &f in fr.keys() {
                if self.held.contains_key(&f) || !seen.insert(f) {
                    continue;
                }
                if let Some(&x) = self.text.get(&f) {
                    xs.push((crate::val::field_name(v, f), f, x));
                }
            }
        }
        xs.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        xs
    }

    fn named(&self, v: &Values) -> Vec<(String, ValueId)> {
        self.sorted(v).into_iter().map(|(n, _, x)| (n, x)).collect()
    }

    /// The routines' values held, as `pgst:<j>` carries them (UNIT: none).
    fn pairs(&self, v: &Values) -> ValueId {
        self.pairs_as(v, true)
    }

    /// [`TexOrder::pairs`]; without the text's own local bindings
    /// (`locals` false): what the text reads at a flush point, the
    /// routines' values alone.
    fn pairs_as(&self, v: &Values, locals: bool) -> ValueId {
        let xs = self.sorted_as(v, locals);
        if xs.is_empty() {
            return ValueId::UNIT;
        }
        v.list(
            xs.into_iter()
                .map(|(_, f, x)| v.list(vec![f.0, x]))
                .collect(),
        )
    }

    fn counts_value(&self, v: &Values) -> ValueId {
        v.list(self.counts.iter().map(|&c| v.int(c)).collect())
    }

    /// The text's assignments `events` in order (`[field, kind, depth,
    /// value]`, kind 0 local, 1 global, 2 changed in place; `[UNIT, 3,
    /// depth]` a group's end; `[UNIT, 4, node, k]` a flush point, its
    /// state noted when `snap`). Whether a count register was assigned.
    fn replay(&mut self, v: &Values, events: &[ValueId], counts: &[Field], snap: bool) -> bool {
        let mut any_count = false;
        for &e in events {
            let xs = list_of(v, e);
            let [f, kind, d, x] = xs[..] else { continue };
            let (kind, d) = (int_of(v, kind), int_of(v, d));
            if f == ValueId::UNIT {
                match kind {
                    3 => {
                        let popped = self.frames.split_off(&d);
                        for (_, fr) in popped.into_iter().rev() {
                            for (g, s) in fr {
                                match s.text {
                                    Some(y) => {
                                        self.text.insert(g, y);
                                    }
                                    None => {
                                        self.text.remove(&g);
                                    }
                                }
                                match s.held {
                                    Some(y) => {
                                        self.held.insert(g, y);
                                    }
                                    None => {
                                        self.held.remove(&g);
                                    }
                                }
                                if let Some(c) = s.count
                                    && let Some(k) = counts.iter().position(|&h| h == g)
                                {
                                    self.counts[k] = c;
                                }
                            }
                        }
                    }
                    4 if snap => {
                        let st = self.pairs_as(v, false);
                        let cs = self.counts_value(v);
                        self.at.insert((d, int_of(v, x)), v.list(vec![cs, st]));
                    }
                    _ => {}
                }
                continue;
            }
            let f = Field(f);
            let k = counts.iter().position(|&h| h == f);
            match kind {
                0 if d > 0 => {
                    let s = Save {
                        held: self.held(f),
                        count: k.map(|k| self.counts[k]),
                        text: self.text.get(&f).copied(),
                    };
                    self.frames.entry(d).or_default().entry(f).or_insert(s);
                }
                1 => self.hide_saves(f),
                _ => {}
            }
            self.held.remove(&f);
            self.text.insert(f, x);
            if let Some(k) = k
                && kind != 2
            {
                any_count = true;
                self.counts[k] = v.as_int(x).unwrap_or(0);
            }
        }
        any_count
    }
}
