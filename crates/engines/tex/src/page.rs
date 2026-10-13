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
        let v = self.v;
        let l = self.list_fold(0)?;
        let items = l.items;
        let marker = self.page_params_marker()?;
        // the insertion classes on the list, read here (before any page
        // step): a `brk` reads nothing a page step may define
        let mut classes = std::collections::BTreeSet::new();
        for &x in &items {
            if nodes::tag(v, x) == nodes::T_INS
                && let Some(Node::Ins(i)) = nodes::decode(v, x, &mut Side::default())
            {
                classes.insert(i.number);
            }
        }
        let mut table = Vec::new();
        for n in classes {
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
        let mut start = PageStart::FIRST;
        let mut marks = MarksState::default();
        let mut published = phitex_defuse::FxHashSet::default();
        let mut starts = vec![start.value(v)];
        // (each page's `\\topmark` and `\\botmark` before it fires,
        // §1012: from the plan, so a step never reads its predecessor's)
        let mut before: Vec<ValueId> = Vec::new();
        let mut ended = false;
        let mut plan: Vec<ValueId> = Vec::new();
        let mut flushplan: Vec<ValueId> = Vec::new();
        let mut reaches: Vec<usize> = Vec::new();
        let mut j = 0;
        while !ended && j < MAX_PAGES {
            let fx = if j > 0 {
                self.page_effects(j - 1)?
            } else {
                Vec::new()
            };
            let out = plan_fold(v, &items, table_v, marker, &start, fx);
            reaches.push(out.reach);
            // the job-written files' order: an immediate write's entry
            // where its marker is, page `j` where it fires (`wplan`)
            plan.extend(out.writes.iter().copied());
            // the flush points met folding this page: each follows the
            // penalty the page before broke at (§1005 fires there, so
            // the marker after it opens this page): the state after
            // page j-1 (none before the first page: UNIT)
            if crate::exec::env_flag!("PHITEX_AUX_TRACE") {
                eprintln!(
                    "aux-trace: plan page {j}: fired {} flushes {:?} stuck {}",
                    out.fired.is_some(),
                    out.flushes,
                    out.stuck
                );
            }
            for &(n, k) in &out.flushes {
                let after = i64::try_from(j).unwrap_or(0) - 1;
                flushplan.push(v.list(vec![v.int(n), v.int(k), v.int(after)]));
            }
            for &node in &out.passed {
                if published.insert(node) {
                    let n = u64::from_ne_bytes(node.to_ne_bytes());
                    let f = v.field(&crate::etex::marks_before_field(n));
                    if self.cx.is_loopback(f) {
                        self.cx.publish(f, marks.value(v, j > 0));
                    }
                }
            }
            if let Some((ms, end)) = out.fired {
                plan.push(v.int(i64::try_from(j).unwrap_or(0)));
                let [t, _, b] = marks.0.get(&0).copied().unwrap_or([None; 3]);
                let u = |m: Option<ValueId>| m.unwrap_or(ValueId::UNIT);
                before.push(v.list(vec![u(t), u(b)]));
                marks.fire_values(v, &ms);
                starts.push(end.value(v));
                start = end;
            } else {
                if out.stuck {
                    self.diag(
                        true,
                        "\\end could not empty the page (an output routine that never ships?)",
                    );
                }
                ended = true;
            }
            j += 1;
        }
        // (the start after the last page folds to the end: no `brk`)
        starts.pop();
        let wp = v.seq_unit(v.list(plan));
        self.contribute("wplan", wp);
        if !flushplan.is_empty() {
            self.contribute("flushplan", v.seq_unit(v.list(flushplan)));
        }
        // each page's own material (`pp:items:<j>`, its start rebased to
        // it): a `brk` reads only its page, so an edit elsewhere on the
        // list wakes no other page's `brk`
        for (j, s) in starts.iter_mut().enumerate() {
            let Some(mut p) = PageStart::of(v, *s) else {
                continue;
            };
            let (a, b) = (p.idx.min(items.len()), reaches[j].min(items.len()));
            self.set(
                &format!("pp:items:{j}"),
                v.list(items[a..b.max(a)].to_vec()),
                true,
            );
            p.idx = 0;
            *s = p.value(v);
        }
        self.set("pp:classes", table_v, true);
        self.set("pp:any", v.int(i64::from(any)), true);
        self.set("pp:marker", marker, true);
        for (j, &s) in starts.iter().enumerate() {
            self.set(&format!("pp:start:{j}"), s, true);
        }
        for (j, &m) in before.iter().enumerate() {
            self.set(&format!("pp:mk:{j}"), m, true);
        }
        // how many steps the chain has: the writer folds their deltas
        // up to here ([`FinalState`])
        self.set(
            "pp:steps",
            v.int(i64::try_from(starts.len()).unwrap_or(0)),
            true,
        );
        for j in 0..starts.len() {
            self.kid(self.ops.brk, vec![v.int(i64::try_from(j).unwrap_or(0))]);
        }
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
        let out = fold_page(v, items, &classes, marker0, &start, fx);
        drop(guard);
        if let Some((f, side, _)) = out.fired {
            let data = self.page_data(&f, &side)?;
            let mk = self.get(&format!("pp:mk:{j}"))?.unwrap_or(ValueId::UNIT);
            // whether the exact PDF writer folds the pages (`pdf:fold`):
            // only then does a step define anything (`pdf:state`)
            let fold = self.cint(crate::fields::cell::PDF_FOLD)?;
            self.kid(
                op,
                vec![v.int(i64::try_from(j).unwrap_or(0)), data, mk, v.int(fold)],
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
        let mut readst = ValueId::UNIT;
        let mut readdef = ValueId::UNIT;
        let mut inherited: Vec<(Field, ValueId)> = Vec::new();
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
        for &(f, x) in &inherited {
            let l = self.loc_of(f);
            self.st.preload(l, Some(x));
        }
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
            for (loc, x, _) in self.st.published(v) {
                let f = self.loc_field(loc);
                let name = crate::val::field_name(v, f);
                if name == "pdf:state" || step_input(&name) {
                    continue;
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
            ]);
            self.contribute("pgdeltas", v.seq_unit(e));
        }
        Ok(())
    }

    /// `\count0..9` read from `cs` (ten values) in place of the
    /// engine's.
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
        if crate::exec::env_flag!("PHITEX_AUX_TRACE") {
            eprintln!(
                "aux-trace: fire page step {} outputpenalty {output_penalty} count0 {:?}",
                self.page_index(),
                self.reg_int(0, 0).ok()
            );
        }
        // §1012: marks
        // (the marks before the page, from the plan: `pp:mk:<j>`)
        let (mut top, bot) = match mk.map(|x| list_of(v, x)) {
            Some(xs) if xs.len() == 2 => {
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
        crate::etex::fire_marks_values(self, &marks)?;
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
    if !left.is_empty() {
        let j = r.page_index();
        if j < MAX_PAGES {
            let x = v.list(left);
            r.cx.publish(r.keyed(crate::fields::Fam::Pagefx, &j.to_string()), x);
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
    /// loop-back `pgst:at:<node>:<k>` (the writer publishes it from the
    /// chain: `[counts, pairs]`), its fields read in place of the
    /// engine's from here on; UNIT (the first round) leaves the state as
    /// it is.
    pub(crate) fn flush_point(&mut self) {
        let v = self.v;
        let op = self.cx.op();
        if op == self.ops.pagestep || op == self.ops.pagestep_any || self.pg.is_some() {
            return;
        }
        let node = self.cx.node().raw();
        let k = self.flushes;
        self.flushes += 1;
        let f = v.field(&format!("pgst:at:{node}:{k}"));
        let decl = v.field(&format!("flushdecl:{node}:{k}"));
        let at = v.list(vec![
            v.int(nodes::T_FLUSH_AT),
            v.int(i64::from_ne_bytes(node.to_ne_bytes())),
            v.int(i64::try_from(k).unwrap_or(0)),
        ]);
        self.contribute("items", v.seq_unit(v.list(vec![v.int(0), at])));
        // (read on the next count-register read, [`Run::flush_read`])
        self.flush_pending = Some((decl, f));
    }

    /// The pending flush point's loop-back read: declared by the client
    /// first (`flushdecl:<node>:<k>`, answered by `api::Graph::settle`;
    /// the node then runs again), then its fields read in place of the
    /// engine's from here on; UNIT (round one) leaves the state as it is.
    pub(crate) fn flush_read(&mut self) -> R<()> {
        let v = self.v;
        let Some((decl, f)) = self.flush_pending.take() else {
            return Ok(());
        };
        if !self.cx.is_loopback(f) {
            self.cx.use_env(decl)?;
            if !self.cx.is_loopback(f) {
                return Ok(());
            }
        }
        let read = self.cx.use_field(f)?;
        if crate::exec::env_flag!("PHITEX_AUX_TRACE") {
            eprintln!(
                "aux-trace: flush point {} reads {}",
                crate::val::field_name(v, f),
                read.map_or("none".to_owned(), |x| v.render(x))
            );
        }
        let Some(x) = read.filter(|&x| x != ValueId::UNIT) else {
            return Ok(());
        };
        let xs = list_of(v, x);
        // (`\count0..9` after the page: not yet; read through, the
        // chain's page number fed back here grows by one a round on
        // LaTeX's `\enddocument` (its `\clearpage` + `\end` pages);
        // `PHITEX_FLUSH_COUNTS=1` tries it)
        if let Some(&cs) = xs.first()
            && crate::exec::env_flag!("PHITEX_FLUSH_COUNTS")
        {
            self.preload_counts(cs);
        }
        if let Some(&pairs) = xs.get(1) {
            for (g, y) in pgst_pairs(v, Some(pairs)) {
                let l = self.loc_of(g);
                self.st.preload(l, Some(y));
            }
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
    flushes: Vec<(i64, i64)>,
    /// The immediate writes' entries met (`T_WRITE_AT`), in order.
    writes: Vec<ValueId>,
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
    flushes: Vec<(i64, i64)>,
    stuck: bool,
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
    flushes: Vec<(i64, i64)>,
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
            stuck: m.stuck,
        };
    }
    let classes = list_of(v, table);
    let out = fold_page(v, items, &classes, marker0, start, fx);
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
) -> PageFold {
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
        let x = if let Some(x) = queue.pop_front() {
            x
        } else if let Some(&x) = items.get(cursor) {
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
        match nodes::tag(v, x) {
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
                flushes.push((crate::val::int_at(v, x, 1), crate::val::int_at(v, x, 2)));
                continue;
            }
            nodes::T_WRITE_AT => {
                if let Some(&e) = list_of(v, x).get(1) {
                    writes.push(e);
                }
                continue;
            }
            _ => {}
        }
        let Some(p) = nodes::decode(v, x, &mut side) else {
            continue;
        };
        // §1000: only a kern looks at what follows it
        let next = if matches!(p, Node::Kern(..)) {
            if queue.is_empty() {
                reach = reach.max((cursor + 1).min(items.len()));
            }
            queue
                .front()
                .copied()
                .or_else(|| items.get(cursor).copied())
                .and_then(|y| nodes::decode(v, y, &mut Side::default()))
        } else {
            None
        };
        if let Outcome::Fire(f) = pb.contribute(p, next.as_ref(), &params, &mut class) {
            let mut carried: Vec<ValueId> = f
                .contributions
                .iter()
                .map(|n| nodes::encode(v, n, &side))
                .collect();
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
        MayDefine::Prefixes(vec!["pp:".to_owned()])
    }
    fn may_define_field(&self, _: &[ValueId], v: &Values, f: Field) -> bool {
        client_only(v, f)
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }
    #[allow(clippy::redundant_closure_for_method_calls)]
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
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
        let args = cx.args();
        run_node(cx, ops(&self.0), |r| r.brk(&args))
    }
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
            MayDefine::Prefixes(vec!["pdf:state".to_owned()])
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
        client_only(v, f) && !matches!(&*v.get(f.0), Value::Str(s) if s.starts_with("pp:"))
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
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
    let r = FinalState::load(cx, o).and_then(|fin| dvi_file(cx, &fin));
    if crate::exec::env_flag!("PHITEX_TEX_TIME") {
        eprintln!(
            "output file (dvi root run): {:?}{}",
            t0.elapsed(),
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
    for page in v.seq_to_vec(pages) {
        dvi_page(v, &mut w, page);
    }
    let bytes = w.finish();
    let s: String = bytes.iter().map(|&b| char::from(b)).collect();
    Ok(v.intern(Value::Str(s.into_boxed_str())))
}

/// One page of the `pages` contribution into `w` (§640).
fn dvi_page(v: &Values, w: &mut phitex_dvi::DviWriter, page: ValueId) {
    let xs = list_of(v, page);
    let mut side = Side::default();
    let Some(Node::Box(bx)) = xs.first().and_then(|&b| nodes::decode(v, b, &mut side)) else {
        return;
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
    let _ = w.ship_out(&bx, counts, &fonts, &mut wh);
}

/// What a DVI page shows: the hash of its `bop`..`eop` bytes written
/// alone, the back pointer left out (it names the previous page's
/// place).
fn dvi_shown(v: &Values, page: ValueId) -> u64 {
    let mut w = phitex_dvi::DviWriter::new(1000, "");
    let at = w.location();
    dvi_page(v, &mut w, page);
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
    ) || name
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
        // read] (`Run::page_step`)
        let entries = Self::entries(v, all);
        let mut refuted: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut state: std::collections::BTreeMap<String, (Field, ValueId)> =
            std::collections::BTreeMap::new();
        let mut cur = ValueId::UNIT;
        // the state after each page, for the flush points' loop-backs
        let mut after: Vec<(ValueId, ValueId)> = Vec::new();
        // `\\count0..9` before the chain, then as folded
        let mut counts = Self::counts(cx)?;
        let ints =
            |x: ValueId| -> Vec<i64> { list_of(v, x).iter().map(|&y| int_of(v, y)).collect() };
        for j in 0..steps {
            let Some(xs) = entries.get(&j) else { continue };
            let g = |i: usize| xs.get(i).copied().unwrap_or(ValueId::UNIT);
            // what step j read against what the steps before it left
            // (step 0 reads the state before the chain itself)
            let read_counts = if j == 0 { counts.clone() } else { ints(g(4)) };
            let read_pairs = pgst_pairs(v, Some(g(2)));
            if j > 0 {
                if g(2) != cur {
                    let read = named(v, &read_pairs);
                    let want: Vec<(String, ValueId)> =
                        state.iter().map(|(n, &(_, x))| (n.clone(), x)).collect();
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
                for (k, (a, b)) in read_counts.iter().zip(&counts).enumerate() {
                    if a != b {
                        refuted.insert(format!("count:{k}"));
                    }
                }
            }
            // `\\count0..9` by the step's change, not its result: the page
            // number the routine advances (`\\advancepageno`, or not on a
            // dead cycle) lands right on every page from one round's runs
            let out_counts = ints(g(3));
            if out_counts.len() == 10 && read_counts.len() == 10 {
                for k in 0..10 {
                    counts[k] += out_counts[k] - read_counts[k];
                }
            }
            if crate::exec::env_flag!("PHITEX_AUX_TRACE") {
                eprintln!(
                    "aux-trace: fold step {j}: read count0 {:?} out {:?} -> {}",
                    read_counts.first(),
                    out_counts.first(),
                    counts[0]
                );
            }
            for (f, x) in pgst_pairs(v, Some(g(1))) {
                let name = crate::val::field_name(v, f);
                let pre = cx.use_field(f)?;
                // a count register by the step's change, as `\\count0..9`
                // (a routine's counter of pages or shipouts it advances)
                let x = if name.starts_with("count:") {
                    let as_int = |y: Option<ValueId>| y.map_or(0, |y| int_of(v, y));
                    let had = as_int(read_pairs.iter().find(|p| p.0 == f).map(|p| p.1).or(pre));
                    let now = as_int(state.get(&name).map(|p| p.1).or(pre));
                    v.int(now + int_of(v, x) - had)
                } else {
                    x
                };
                if same_as_before(v, &name, pre, x) {
                    state.remove(&name);
                } else {
                    state.insert(name, (f, x));
                }
            }
            cur = if state.is_empty() {
                ValueId::UNIT
            } else {
                v.list(state.values().map(|&(f, x)| v.list(vec![f.0, x])).collect())
            };
            let e: Vec<(String, ValueId)> =
                state.iter().map(|(n, &(_, x))| (n.clone(), x)).collect();
            log_pub(o, "pgst", j, e);
            let jj = v.int(i64::try_from(j).unwrap_or(0));
            if cur != ValueId::UNIT {
                cx.publish(v.field(&format!("pgst:{j}")), cur);
                // (kept for the client: the next session's guess)
                cx.contribute(
                    v.field("pgsts"),
                    v.seq_unit(v.list(vec![v.int(0), jj, cur])),
                );
            }
            let cs = v.list(counts.iter().map(|&c| v.int(c)).collect());
            log_pub(o, "pgdef", j, count_entries(v, Some(cs)));
            cx.publish(v.field(&format!("pgdef:{j}")), cs);
            cx.contribute(v.field("pgsts"), v.seq_unit(v.list(vec![v.int(1), jj, cs])));
            after.resize(j + 1, (ValueId::UNIT, ValueId::UNIT));
            after[j] = (cs, cur);
        }
        Self::log_refuted(o, refuted);
        // the flush points (`flushplan`: `[node, k, page]`): the state
        // after that page, UNIT before the first
        let fp = cx.acc_range(v.field("flushplan"), None)?;
        for e in v.seq_to_vec(fp).iter().flat_map(|&e| list_of(v, e)) {
            let xs = list_of(v, e);
            let [n, k, p] = xs.as_slice() else { continue };
            // (the node id as the marker carries it: a signed value)
            let node = u64::from_ne_bytes(int_of(v, *n).to_ne_bytes());
            let f = v.field(&format!("pgst:at:{node}:{}", int_of(v, *k)));
            if !cx.is_loopback(f) {
                continue;
            }
            let x = usize::try_from(int_of(v, *p))
                .ok()
                .and_then(|p| after.get(p))
                .map_or(ValueId::UNIT, |&(cs, st)| v.list(vec![cs, st]));
            if crate::exec::env_flag!("PHITEX_AUX_TRACE") {
                eprintln!(
                    "aux-trace: flush point {node}:{} after page {}: {} (before the chain {counts:?})",
                    int_of(v, *k),
                    int_of(v, *p),
                    v.render(x)
                );
            }
            cx.publish(f, x);
        }
        Ok(FinalState {
            map: state.into_values().collect(),
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

    /// `\\count0..9` before the chain.
    fn counts(cx: &mut Ctx<'_>) -> Result<Vec<i64>, Stop> {
        let v = cx.values();
        let mut counts: Vec<i64> = Vec::with_capacity(10);
        for k in 0..10 {
            counts.push(
                cx.use_field(v.field(&format!("count:{k}")))?
                    .map_or(0, |x| int_of(v, x)),
            );
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
