//! The page builder and `\output` (§980–§1028), `\shipout` (§638) and
//! the DVI file (§583–§645).
//!
//! The page builder is a fold over the main vertical list: each node that
//! calls `build_page` in tex.web (§1035 and the rest) is a step. It reads
//! the field `page:state` (the builder's state as a value: its key, how
//! many of list 0's items it has taken, and the contributions waiting
//! before them), contributes what the list gained since, and defines the
//! new state. When a page fires, the output routine is the step's
//! unfolding: an `OutPage` child opens the output group with `\box255`
//! and the marks defined, an `Exec` child holds the routine's tokens, and
//! the group's `}` (`output_end`) is again a step of the fold.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock, PoisonError};

use phitex_defuse::{Ctx, Stop, Value, ValueId, Values};
use phitex_typeset::glue::Glue;
use phitex_typeset::node::{List, Node};
use phitex_typeset::pack::{PackParams, Spec};
use phitex_typeset::page::{InsertClass, Outcome, PageBuilder, PageParams};

use crate::build::Mode;
use crate::nodes::{self, Side};
use crate::run::{Halt, R, Run};
use crate::val::{group, int_of, list_of, str_of, toks_of};

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

    /// The page builder's state where this node is.
    pub(crate) fn page_state(&mut self) -> R<State> {
        // a read of the page state folds a due step first
        self.flush_page()?;
        let x = self.get("page:state")?;
        Ok(State::decode(self.v, x))
    }

    /// The fold over the main list's contributions up to the last
    /// `build_page` call's marker (what the same run appended after it
    /// waits for the next).
    pub(crate) fn page_fold(&mut self) -> R<()> {
        let mut st = self.page_state()?;
        let l = self.list_fold(0)?;
        let end = l.bp_at;
        let mut queue: VecDeque<ValueId> = st.pending.drain(..).collect();
        queue.extend(l.items[..end].iter().skip(st.consumed).copied());
        st.consumed = end.max(st.consumed);
        self.page_steps(st, queue)
    }

    /// Contributes `queue` to the page: until it is empty or a page fires
    /// with an output routine to run.
    fn page_steps(&mut self, mut st: State, mut queue: VecDeque<ValueId>) -> R<()> {
        let v = self.v;
        let params = PageParams {
            vsize: self.param("vsize")?,
            max_depth: self.param("maxdepth")?,
            top_skip: self.glue_param("topskip")?,
            holding_inserts: self.param("holdinginserts")?,
            saving_vdiscards: false,
        };
        let (mut pb, mut side) = builder(st.key);
        while let Some(x) = queue.pop_front() {
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
                    // a kern waits for what follows it (§1000)
                    st.pending = vec![nodes::encode(v, &k, &side)];
                    st.pending.extend(queue);
                    st.key = remember(pb, side);
                    self.set("page:state", st.encode(v), true);
                    return Ok(());
                }
                Outcome::Fire(f) => {
                    let mut rest: Vec<ValueId> = f
                        .contributions
                        .iter()
                        .map(|n| nodes::encode(v, n, &side))
                        .collect();
                    rest.extend(queue);
                    st.pending = rest;
                    st.key = remember(pb, side.clone());
                    if self.fire(&st, &f, &side)? {
                        // the output routine runs; its end steps the fold
                        return Ok(());
                    }
                    let pending = std::mem::take(&mut st.pending);
                    queue = pending.into();
                    let (p2, s2) = builder(st.key);
                    pb = p2;
                    side = s2;
                }
            }
        }
        st.key = remember(pb, side);
        self.set("page:state", st.encode(v), true);
        Ok(())
    }

    /// `fire_up` (§1012–§1025) after the page builder chose the page:
    /// true if an output routine runs (as children).
    fn fire(&mut self, st: &State, f: &phitex_typeset::page::Fired, side: &Side) -> R<bool> {
        let v = self.v;
        // §1012: marks
        let bot = self.get("mark:bot")?.filter(|&x| x != ValueId::UNIT);
        let mut top = self.get("mark:top")?.filter(|&x| x != ValueId::UNIT);
        let mut first = None;
        let mut new_bot = bot;
        if bot.is_some() {
            top = bot;
        }
        crate::etex::fire_marks(self, &f.marks, side)?;
        for &k in &f.marks {
            let m = side.marks.get(k as usize).copied();
            if crate::etex::is_class_mark(v, m) {
                continue;
            }
            if first.is_none() {
                first = m;
            }
            new_bot = m;
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
        for (n, list) in &f.inserts {
            let old = self.box_reg(i32::from(*n))?;
            let mut s2 = Side::default();
            let mut l: List = match old.and_then(|b| nodes::decode(v, b, &mut s2)) {
                Some(Node::Box(b)) => b.list,
                _ => Vec::new(),
            };
            l.extend(list.iter().cloned());
            let p = phitex_typeset::pack::vpack(l, Spec::Additional(0), &PackParams::default());
            let merged = merge(s2, side);
            let b = nodes::encode_box(v, &p.node, &merged);
            self.set(&format!("box:{n}"), v.list(vec![v.int(0), b]), true);
        }
        let b255 = nodes::encode_box(v, &f.box255.node, side);
        let held: Vec<ValueId> = f
            .held_inserts
            .iter()
            .map(|n| nodes::encode(v, n, side))
            .collect();
        self.set(
            "param:outputpenalty",
            v.int(i64::from(f.output_penalty)),
            true,
        );
        let output = self.get("param:output")?;
        let otoks = toks_of(v, output);
        if otoks.is_empty() {
            // §1023: the default output routine
            self.ship_out(b255)?;
            let mut st2 = st.clone();
            let mut q = held;
            q.append(&mut st2.pending);
            st2.pending = q;
            self.set("page:state", st2.encode(v), true);
            return Ok(false);
        }
        // §1025
        let dead = self.int("deadcycles")? + 1;
        self.set("deadcycles", v.int(dead), true);
        self.set("page:state", st.encode(v), true);
        let op = self.ops.outpage;
        self.kid(op, vec![b255, v.list(held)]);
        let mut toks = otoks;
        if !toks.is_empty() {
            toks.remove(0);
        }
        let frame = v.list(vec![v.str("t"), crate::val::toks_value(v, &toks)]);
        let exec = self.ops.exec;
        let base = v.int(crate::run::OUTPUT_IF_BASE);
        let z = v.int(0);
        self.kid(exec, vec![v.list(vec![frame]), z, z, z, base]);
        Ok(true)
    }

    /// `\pagegoal` and the other page quantities (§421): the fold's state
    /// where this node is.
    pub(crate) fn page_dimen(&mut self, m: i32) -> R<i32> {
        let st = self.page_state()?;
        let (pb, _) = builder(st.key);
        Ok(
            if pb.contents == phitex_typeset::page::Contents::Empty && m == 0 {
                MAX_DIMEN
            } else {
                pb.so_far[usize::try_from(m).unwrap_or(0).min(7)]
            },
        )
    }

    /// Whether the page and its contributions are empty (§1054).
    pub(crate) fn page_empty(&mut self) -> R<bool> {
        let st = self.page_state()?;
        let (items, _) = self.list_items(0)?;
        let (pb, _) = builder(st.key);
        Ok(pb.page.is_empty() && st.pending.is_empty() && items.len() <= st.consumed)
    }
}

const MAX_DIMEN: i32 = 0o7777777777;

/// The fold's state as a value: the builder's key (a pure function's
/// memo, [`builder`]), the items of list 0 it has taken, and what waits
/// before the rest.
#[derive(Clone, Debug, Default)]
pub(crate) struct State {
    pub key: u64,
    pub consumed: usize,
    pub pending: Vec<ValueId>,
}

impl State {
    fn decode(v: &phitex_defuse::Values, x: Option<ValueId>) -> State {
        let Some(x) = x else { return State::default() };
        let xs = list_of(v, x);
        State {
            key: u64::from_ne_bytes(xs.first().map_or(0, |&k| int_of(v, k)).to_ne_bytes()),
            consumed: usize::try_from(xs.get(1).map_or(0, |&k| int_of(v, k))).unwrap_or(0),
            pending: xs.get(2).map(|&p| list_of(v, p)).unwrap_or_default(),
        }
    }

    fn encode(&self, v: &phitex_defuse::Values) -> ValueId {
        v.list(vec![
            v.int(i64::from_ne_bytes(self.key.to_ne_bytes())),
            v.int(i64::try_from(self.consumed).unwrap_or(0)),
            v.list(self.pending.clone()),
        ])
    }
}

type Builders = Mutex<HashMap<u64, (PageBuilder, Side)>>;

fn builders() -> &'static Builders {
    static B: OnceLock<Builders> = OnceLock::new();
    B.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The builder a key names (0: the empty page).
fn builder(key: u64) -> (PageBuilder, Side) {
    if key == 0 {
        return (PageBuilder::new(), Side::default());
    }
    builders()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .cloned()
        .unwrap_or_else(|| (PageBuilder::new(), Side::default()))
}

/// The key of a builder: a hash of its content, so the same page gives
/// the same state value whatever ran before.
fn remember(pb: PageBuilder, side: Side) -> u64 {
    let k = phitex_defuse::hash_bytes(
        17,
        format!("{pb:?}|{:?}|{:?}", side.marks, side.whats).as_bytes(),
    );
    let k = if k == 0 { 1 } else { k };
    builders()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(k)
        .or_insert((pb, side));
    k
}

fn merge(a: Side, b: &Side) -> Side {
    let mut s = Side {
        marks: b.marks.clone(),
        whats: b.whats.clone(),
    };
    s.marks.extend(a.marks);
    s.whats.extend(a.whats);
    s
}

/// `OutPage`: §1025, the output routine begins: `\box255` defined, the
/// output group opened (its kind keeps the held-over insertions).
pub(crate) fn out_page(r: &mut Run<'_, '_>, args: &[ValueId]) -> R<()> {
    let v = r.v;
    let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
    r.set("box:255", v.list(vec![v.int(0), a(0)]), true);
    let md = r.mode()?;
    let sid = r.node_raw();
    let kind = r.kind_value(&md, group::OUTPUT, sid, sid, -1, vec![a(1)])?;
    r.open_scope_here(kind);
    r.normal_paragraph()?;
    Ok(())
}

/// The output group's `}` (§1026): the routine's list goes after the
/// held-over insertions, both before the contributions the page left;
/// then a step of the fold.
pub(crate) fn output_end(r: &mut Run<'_, '_>, md: &Mode) -> R<()> {
    let v = r.v;
    r.effect = phitex_defuse::Effect::Close;
    r.unsave_extras(md)?;
    let (items, _) = r.list_items(md.sid)?;
    let held = md.extra.first().map(|&x| list_of(v, x)).unwrap_or_default();
    let mut st = r.page_state()?;
    let mut q = held;
    q.extend(items);
    q.append(&mut st.pending);
    r.set("inspen", v.int(0), true);
    let outer = r.outer_mode(md)?;
    let _ = outer;
    r.page_steps(st, q.into())
}

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
