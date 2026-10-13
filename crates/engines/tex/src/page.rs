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
//! steps read nothing of each other but what the routine defines
//! globally (`\count0`, the insert boxes); no state cell threads them.
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
use crate::nodes::{self, Side};
use crate::ops::{Shared, client_only, ops, run_node};
use crate::run::{Halt, R, Run};
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
            *c = self.int32(&format!("count:{k}"))?;
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
                    self.open_out(n, &name);
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
        self.set("deadcycles", v.int(0), true);
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
        xs.push(self.get("pdf:colorstacks")?.unwrap_or(ValueId::UNIT));
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

    fn insert_class(&mut self, n: u16) -> R<InsertClass> {
        let b = self.box_reg(i32::from(n))?;
        let v = self.v;
        Ok(InsertClass {
            box_height_plus_depth: b.map(|b| nodes::box_dim(v, b, 3) + nodes::box_dim(v, b, 2)),
            box_is_hbox: b.is_some_and(|b| nodes::box_is_h(v, b)),
            count: self.int32(&format!("count:{n}"))?,
            dimen: self.int32(&format!("dimen:{n}"))?,
            skip: crate::val::glue_of(v, self.get(&format!("skip:{n}"))?),
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

    /// The page builder's fold (`Breaks`): every page's material, as the
    /// children that run the output routine.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn breaks(&mut self) -> R<()> {
        let v = self.v;
        let l = self.list_fold(0)?;
        let mut params = PageParams {
            vsize: self.param("vsize")?,
            max_depth: self.param("maxdepth")?,
            top_skip: self.glue_param("topskip")?,
            holding_inserts: self.param("holdinginserts")?,
            saving_vdiscards: false,
        };
        let mut queue: VecDeque<ValueId> = l.items.into_iter().collect();
        let mut pb = PageBuilder::new();
        let mut side = Side::default();
        let mut pages: Vec<ValueId> = Vec::new();
        // §1054: `\end` appends its material until the page is empty
        let mut ending: Option<ValueId> = None;
        let mut deadcycles = 0u32;
        // the marks in force after each fired page (§1012–§1014), by
        // class: what a document node reading them at a later point sees
        let mut marks = MarksState::default();
        let mut published = std::collections::HashSet::new();
        loop {
            let Some(x) = queue.pop_front() else {
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
                    // §1054: an empty page (and empty contributions: the
                    // queue holds nothing before `\end`) ends the job
                    // without another cycle
                    if pb.page.is_empty() {
                        continue;
                    }
                    let xs = list_of(v, x);
                    for &y in xs.iter().skip(1).rev() {
                        queue.push_front(y);
                    }
                    continue;
                }
                nodes::T_PAGE_PARAMS => {
                    let xs = list_of(v, x);
                    let n = |i: usize| {
                        i32::try_from(xs.get(i).map_or(0, |&y| int_of(v, y))).unwrap_or(0)
                    };
                    params.vsize = n(1);
                    params.max_depth = n(2);
                    params.top_skip = crate::val::glue_of(v, xs.get(3).copied());
                    params.holding_inserts = n(4);
                    continue;
                }
                nodes::T_MARKS_AT => {
                    let node = crate::val::int_at(v, x, 1);
                    if published.insert(node) {
                        let n = u64::from_ne_bytes(node.to_ne_bytes());
                        let f = v.field(&crate::etex::marks_before_field(n));
                        if self.cx.is_loopback(f) {
                            self.cx.publish(f, marks.value(v, !pages.is_empty()));
                        }
                    }
                    continue;
                }
                _ => {}
            }
            let Some(p) = nodes::decode(v, x, &mut side) else {
                continue;
            };
            let next = queue
                .front()
                .and_then(|&y| nodes::decode(v, y, &mut Side::default()));
            let mut halt: Option<Halt> = None;
            let out = {
                let mut class = |n: u16| match self.insert_class(n) {
                    Ok(c) => c,
                    Err(h) => {
                        halt.get_or_insert(h);
                        InsertClass {
                            box_height_plus_depth: None,
                            box_is_hbox: false,
                            count: 0,
                            dimen: 0,
                            skip: Glue::ZERO,
                        }
                    }
                };
                pb.contribute(p, next.as_ref(), &params, &mut class)
            };
            if let Some(h) = halt {
                return Err(h);
            }
            match out {
                Outcome::Done => {}
                Outcome::Wait(k) => {
                    // a kern waits for what follows it (§1000): nothing
                    // does, the list is whole
                    let _ = k;
                }
                Outcome::Fire(f) => {
                    let j = pages.len();
                    // the material the fold could not place goes on
                    // (§1017); what the routine leaves is next round's
                    let fx = self.page_effects(j)?;
                    let mut rest: Vec<ValueId> = fx;
                    rest.extend(f.contributions.iter().map(|n| nodes::encode(v, n, &side)));
                    for y in rest.into_iter().rev() {
                        queue.push_front(y);
                    }
                    marks.fire(v, &f.marks, &side);
                    let data = self.page_data(&f, &side)?;
                    pages.push(data);
                    pb = PageBuilder::new();
                    side = Side::default();
                }
            }
        }
        if ending.is_some() && !pb.page.is_empty() {
            self.diag(
                true,
                "\\end could not empty the page (an output routine that never ships?)",
            );
        }
        let output = self.get("param:output")?;
        let any = toks_of(v, output).is_empty();
        let op = if any {
            self.ops.pagestep_any
        } else {
            self.ops.pagestep
        };
        for (j, d) in pages.into_iter().enumerate() {
            self.kid(op, vec![v.int(i64::try_from(j).unwrap_or(0)), d]);
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
        let f = v.field(&format!("pagefx:{j}"));
        if !self.cx.is_loopback(f) {
            return Ok(Vec::new());
        }
        Ok(self
            .cx
            .use_field(f)?
            .map(|x| list_of(v, x))
            .unwrap_or_default())
    }

    /// A fired page as the step's argument: `[box255, inserts, marks,
    /// held, output_penalty]`, the marks as values (§1012–§1021).
    #[allow(clippy::unnecessary_wraps)]
    fn page_data(&mut self, f: &phitex_typeset::page::Fired, side: &Side) -> R<ValueId> {
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
        let data = args.get(1).map(|&d| list_of(v, d)).unwrap_or_default();
        let a = |i: usize| data.get(i).copied().unwrap_or(ValueId::UNIT);
        let (b255, inserts, marks, held) =
            (a(0), list_of(v, a(1)), list_of(v, a(2)), list_of(v, a(3)));
        let output_penalty = data.get(4).map_or(0, |&x| int_of(v, x));
        // §1012: marks
        let bot = self.get("mark:bot")?.filter(|&x| x != ValueId::UNIT);
        let mut top = self.get("mark:top")?.filter(|&x| x != ValueId::UNIT);
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
        let output = self.get("param:output")?;
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
            return Ok(());
        }
        // §1025
        let dead = self.int("deadcycles")? + 1;
        self.set("deadcycles", v.int(dead), true);
        self.set("box:255", v.list(vec![v.int(0), b255]), true);
        self.push_list(-1, group::OUTPUT, vec![v.list(held)]);
        self.normal_paragraph()?;
        let mut toks = otoks;
        if !toks.is_empty() {
            toks.remove(0);
        }
        self.push_toks(toks);
        self.run_all()
    }
}

/// The output group's `}` (§1026): the routine's list and the held-over
/// insertions are what the next page sees first (`pagefx:<j>`, next
/// round); `\box255` must be void.
pub(crate) fn output_end(r: &mut Run<'_, '_>, md: &Mode) -> R<()> {
    let v = r.v;
    let (items, _) = r.list_items(md.lid)?;
    let held = md.extra.first().map(|&x| list_of(v, x)).unwrap_or_default();
    r.close_group(md)?;
    if r.box_reg(255)?.is_some() {
        r.log("error", "Output routine didn't use all of \\box255")?;
    }
    r.set("inspen", v.int(0), true);
    let mut left = held;
    left.extend(items);
    if !left.is_empty() {
        let j = r.page_index();
        if j < MAX_PAGES {
            r.cx.publish(v.field(&format!("pagefx:{j}")), v.list(left));
        }
    }
    r.set("deadcycles", v.int(0), true);
    Ok(())
}

impl Run<'_, '_> {
    /// Which page this step is (its first argument), or `MAX_PAGES`.
    fn page_index(&self) -> usize {
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
    /// A page fires with marks `ks` (numbers into `side.marks`).
    fn fire(&mut self, v: &Values, ks: &[u32], side: &Side) {
        let page: Vec<(i32, ValueId)> = ks
            .iter()
            .filter_map(|&k| side.marks.get(k as usize).copied())
            .map(|m| crate::etex::mark_class(v, m))
            .collect();
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

/// `Breaks`: the page builder's fold over the main vertical list.
pub(crate) struct Breaks(pub Shared);

impl Operation for Breaks {
    fn name(&self) -> &'static str {
        "breaks"
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
    #[allow(clippy::redundant_closure_for_method_calls)]
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        run_node(cx, ops(&self.0), |r| r.breaks())
    }
}

/// One page's step: the output routine run, the page shipped. With an
/// output routine the step defines the marks, the insertion boxes,
/// `\box255`, `\outputpenalty`, `\deadcycles` and whatever the routine
/// defines globally; without one (§1023) it ships the page itself.
pub(crate) struct PageStep {
    pub shared: Shared,
    pub any: bool,
}

impl Operation for PageStep {
    fn name(&self) -> &'static str {
        if self.any { "pagestep_any" } else { "pagestep" }
    }
    fn may_define(&self, _: &[ValueId], _: &Values) -> MayDefine {
        MayDefine::Any
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn may_define_field(&self, _: &[ValueId], v: &Values, f: Field) -> bool {
        client_only(v, f)
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let args = cx.args();
        run_node(cx, ops(&self.shared), |r| r.page_step(&args))
    }
}

const MAX_DIMEN: i32 = 0o7777777777;

/// `Dvi`: the pages, written (§583–§645). The comment and `\mag` are
/// read at the end.
pub(crate) fn dvi(cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
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
        if crate::pdfrelaxed::finishing(cx)? {
            return crate::pdfrelaxed::finish(cx);
        }
        return crate::pdfout::finish(cx);
    }
    let comment = cx
        .use_field(v.field("dvicomment"))?
        .map(|x| str_of(v, x))
        .unwrap_or_default();
    let mag = cx
        .use_field(v.field("param:mag"))?
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
    let mut defs = std::collections::HashMap::new();
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
