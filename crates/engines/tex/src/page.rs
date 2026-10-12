//! The page builder and `\output` (§980–§1028), `\shipout` (§638) and
//! the DVI file (§583–§645).
//!
// REDUCE-GAP: resumable fold. The page builder is a fold over the main
// vertical list; the core has no fold that can stop, run a subprogram
// (`\output`) and resume. It is stood in for by `Fin`, a root at the
// end of the document that reads the whole main list and builds pages
// until one fires; `\output` then runs as children of `Fin` (`OutPage`
// opens the output group, an `Exec` child reads the output routine),
// and the group's `}` (`output_end`) makes a new `Fin` child with what
// is left to contribute. Every page is recomputed on any change of the
// main list. Commands between two pages of the document do not see
// what the first page's `\output` did (`\pageno`, marks): it runs at
// the end.
//
// REDUCE-GAP: `\pagegoal`, `\pagetotal` and the other page quantities
// read during the document are those of an empty page: the page
// builder runs after the document.

use phitex_defuse::{Ctx, Stop, Value, ValueId};
use phitex_typeset::glue::Glue;
use phitex_typeset::node::{BoxKind, BoxNode, GlueKind, List, Node};
use phitex_typeset::pack::{PackParams, Spec};
use phitex_typeset::page::{InsertClass, Outcome, PageBuilder, PageParams};

use crate::build::Mode;
use crate::nodes::{self, Side};
use crate::run::{Halt, R, Run};
use crate::val::{Tok, group, int_of, list_of, str_of, toks_of};

const MAX_DEAD_CYCLES_GUARD: i64 = 1000;

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
        // §1366: the whatsits in the order `hlist_out`/`vlist_out` meet them
        for w in side.whats.clone() {
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
                    let x = v.list(vec![v.str(&name)]);
                    self.set(&format!("write:{n}"), x, true);
                }
                "close" => {
                    let n = xs.get(1).map_or(0, |&y| int_of(v, y));
                    self.set(&format!("write:{n}"), ValueId::UNIT, true);
                }
                _ => {}
            }
        }
        let view = self.font_view(std::slice::from_ref(&Node::Box(bx)))?;
        let mut fonts = Vec::new();
        let mut ids: Vec<u32> = view.fonts.keys().copied().collect();
        ids.sort_unstable();
        for f in ids {
            let info = self.get(&format!("fontinfo:{f}"))?;
            let Some(i) = crate::fonts::info(v, info) else { continue };
            let Some(file) = i.file else { continue };
            fonts.push(v.list(vec![v.int(i64::from(f)), v.str(&i.name), file, v.int(i64::from(i.s))]));
        }
        let cs = v.list(counts.iter().map(|&c| v.int(i64::from(c))).collect());
        let page = v.list(vec![b, cs, v.list(fonts)]);
        let su = v.seq_unit(page);
        self.contribute("pages", su);
        self.log("shipend", "]")?;
        self.set("deadcycles", v.int(0), true);
        Ok(())
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
}

/// The main vertical list as TeX's contributions: list 0's items, the
/// deletions applied.
fn main_list(r: &mut Run<'_, '_>) -> R<Vec<ValueId>> {
    let (items, _) = r.list_items(0)?;
    Ok(items)
}

/// `Fin`: the page builder over what is left of the main vertical list
/// (§994–§1028), and `its_all_over` (§1054) once it is empty.
#[allow(clippy::too_many_lines)]
pub(crate) fn fin(r: &mut Run<'_, '_>, args: &[ValueId]) -> R<()> {
    let v = r.v;
    if r.int("ended")? != 1 {
        return Ok(());
    }
    let mut queue: std::collections::VecDeque<ValueId> = match args.first() {
        Some(&q) => list_of(v, q).into(),
        None => main_list(r)?.into(),
    };
    let params = PageParams {
        vsize: r.param("vsize")?,
        max_depth: r.param("maxdepth")?,
        top_skip: r.glue_param("topskip")?,
        holding_inserts: r.param("holdinginserts")?,
        saving_vdiscards: false,
    };
    let mut side = Side::default();
    let mut pb = PageBuilder::new();
    let mut guard = 0;
    loop {
        let Some(x) = queue.pop_front() else {
            // §1054 its_all_over
            let dead = r.int("deadcycles")?;
            if pb.page.is_empty() && dead == 0 {
                return Ok(());
            }
            guard += 1;
            if guard > MAX_DEAD_CYCLES_GUARD {
                return Ok(());
            }
            let w = r.param("hsize")?;
            let b = BoxNode {
                kind: BoxKind::H,
                width: w,
                ..BoxNode::default()
            };
            let fill = Glue {
                stretch: 65536,
                stretch_order: phitex_typeset::glue::Order::Fill,
                ..Glue::ZERO
            };
            queue.push_back(nodes::encode_box(v, &b, &side));
            queue.push_back(nodes::glue_node(v, &fill, GlueKind::Normal));
            queue.push_back(nodes::penalty_node(v, -0o10000000000));
            continue;
        };
        let Some(p) = nodes::decode(v, x, &mut side) else { continue };
        let next = queue.front().and_then(|&y| {
            let mut s2 = Side::default();
            nodes::decode(v, y, &mut s2)
        });
        let mut halt: Option<Halt> = None;
        let out = {
            let mut class = |n: u16| match r.insert_class(n) {
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
                if queue.is_empty() {
                    // contributed again once the end appends its box
                    let k = nodes::encode(v, &k, &side);
                    let w = r.param("hsize")?;
                    let b = BoxNode {
                        kind: BoxKind::H,
                        width: w,
                        ..BoxNode::default()
                    };
                    let _ = b;
                    queue.push_front(k);
                    // its_all_over's material follows the kern
                    let dead = r.int("deadcycles")?;
                    let _ = dead;
                    let fill = Glue {
                        stretch: 65536,
                        stretch_order: phitex_typeset::glue::Order::Fill,
                        ..Glue::ZERO
                    };
                    let bx = BoxNode {
                        kind: BoxKind::H,
                        width: r.param("hsize")?,
                        ..BoxNode::default()
                    };
                    queue.push_back(nodes::encode_box(v, &bx, &side));
                    queue.push_back(nodes::glue_node(v, &fill, GlueKind::Normal));
                    queue.push_back(nodes::penalty_node(v, -0o10000000000));
                } else {
                    let k = nodes::encode(v, &k, &side);
                    queue.push_front(k);
                }
            }
            Outcome::Fire(f) => return fire(r, *f, &side, queue),
        }
    }
}

/// `fire_up` (§1012–§1025), after the page builder chose the page.
fn fire(
    r: &mut Run<'_, '_>,
    f: phitex_typeset::page::Fired,
    side: &Side,
    rest: std::collections::VecDeque<ValueId>,
) -> R<()> {
    let v = r.v;
    // §1012: marks
    let bot = r.get("mark:bot")?.filter(|&x| x != ValueId::UNIT);
    let mut top = r.get("mark:top")?.filter(|&x| x != ValueId::UNIT);
    let mut first = None;
    let mut new_bot = bot;
    if bot.is_some() {
        top = bot;
    }
    for &k in &f.marks {
        let m = side.marks.get(k as usize).copied();
        if first.is_none() {
            first = m;
        }
        new_bot = m;
    }
    if top.is_some() && first.is_none() {
        first = top;
    }
    let marks = v.list(vec![
        top.unwrap_or(ValueId::UNIT),
        first.unwrap_or(ValueId::UNIT),
        new_bot.unwrap_or(ValueId::UNIT),
    ]);
    // §1018–§1021: insertion boxes
    let mut ins = Vec::new();
    for (n, list) in &f.inserts {
        let old = r.box_reg(i32::from(*n))?;
        let mut s2 = Side::default();
        let mut l: List = match old.and_then(|b| nodes::decode(v, b, &mut s2)) {
            Some(Node::Box(b)) => b.list,
            _ => Vec::new(),
        };
        l.extend(list.iter().cloned());
        let p = phitex_typeset::pack::vpack(l, Spec::Additional(0), &PackParams::default());
        let merged = merge(s2, side);
        ins.push(v.list(vec![v.int(i64::from(*n)), nodes::encode_box(v, &p.node, &merged)]));
    }
    let b255 = nodes::encode_box(v, &f.box255.node, side);
    let held = nodes::encode_list(v, &f.held_inserts, side);
    let mut cont: Vec<ValueId> = f.contributions.iter().map(|n| nodes::encode(v, n, side)).collect();
    cont.extend(rest);
    let cont = v.list(cont);
    let output = r.get("param:output")?;
    let otoks = toks_of(v, output);
    if otoks.is_empty() {
        // §1023: the default output routine
        for x in list_of(v, ins_list(v, &ins)) {
            let xs = list_of(v, x);
            let n = xs.first().map_or(0, |&y| int_of(v, y));
            let val = v.list(vec![v.int(0), xs[1]]);
            r.set(&format!("box:{n}"), val, true);
        }
        set_marks(r, marks);
        r.set("param:outputpenalty", v.int(i64::from(f.output_penalty)), true);
        r.ship_out(b255)?;
        let mut q = list_of(v, held);
        q.extend(list_of(v, cont));
        let fin = r.ops.fin;
        r.kid(fin, vec![v.list(q)]);
        return Ok(());
    }
    let dead = r.int("deadcycles")? + 1;
    let out = OutArgs {
        b255,
        penalty: f.output_penalty,
        marks,
        ins: ins_list(v, &ins),
        held,
        cont,
        dead,
    };
    let op = r.ops.outpage;
    r.kid(op, out.encode(v, &otoks));
    Ok(())
}

fn ins_list(v: &phitex_defuse::Values, ins: &[ValueId]) -> ValueId {
    v.list(ins.to_vec())
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

fn set_marks(r: &mut Run<'_, '_>, marks: ValueId) {
    let v = r.v;
    let xs = list_of(v, marks);
    for (i, name) in ["mark:top", "mark:first", "mark:bot"].iter().enumerate() {
        let m = xs.get(i).copied().unwrap_or(ValueId::UNIT);
        r.set(name, m, true);
    }
}

/// What `OutPage` is given.
struct OutArgs {
    b255: ValueId,
    penalty: i32,
    marks: ValueId,
    ins: ValueId,
    held: ValueId,
    cont: ValueId,
    dead: i64,
}

impl OutArgs {
    fn encode(&self, v: &phitex_defuse::Values, toks: &[Tok]) -> Vec<ValueId> {
        vec![
            self.b255,
            v.int(i64::from(self.penalty)),
            self.marks,
            self.ins,
            self.held,
            self.cont,
            v.int(self.dead),
            crate::val::toks_value(v, toks),
        ]
    }
}

/// `OutPage`: §1025, the output routine begins: the page in `\box255`,
/// the output group opened (its kind keeps what the page builder
/// resumes with), and the routine's tokens read by an `Exec` child.
pub(crate) fn out_page(r: &mut Run<'_, '_>, args: &[ValueId]) -> R<()> {
    let v = r.v;
    let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
    r.set("box:255", v.list(vec![v.int(0), a(0)]), true);
    r.set("param:outputpenalty", a(1), true);
    set_marks(r, a(2));
    for x in list_of(v, a(3)) {
        let xs = list_of(v, x);
        let n = xs.first().map_or(0, |&y| int_of(v, y));
        r.set(&format!("box:{n}"), v.list(vec![v.int(0), xs[1]]), true);
    }
    r.set("deadcycles", a(6), true);
    r.set("ended", v.int(0), true);
    let md = r.mode()?;
    let sid = r.node_raw();
    let kind = r.kind_value(&md, group::OUTPUT, sid, sid, -1, vec![a(4), a(5)])?;
    r.open_scope_here(kind);
    r.normal_paragraph()?;
    // the routine without its `{` (scan_left_brace, §1025)
    let mut toks = toks_of(v, Some(a(7)));
    if !toks.is_empty() {
        toks.remove(0);
    }
    let frame = v.list(vec![v.str("t"), crate::val::toks_value(v, &toks)]);
    let exec = r.ops.exec;
    r.kid(exec, vec![v.list(vec![frame])]);
    Ok(())
}

/// The output group's `}` (§1026): the routine's list goes after the
/// held-over insertions, both before the contributions the page left,
/// and the page builder resumes.
pub(crate) fn output_end(r: &mut Run<'_, '_>, md: &Mode) -> R<()> {
    let v = r.v;
    r.effect = phitex_defuse::Effect::Close;
    r.unsave_extras(md)?;
    let (items, _) = r.list_items(md.sid)?;
    let held = md.extra.first().map(|&x| list_of(v, x)).unwrap_or_default();
    let cont = md.extra.get(1).map(|&x| list_of(v, x)).unwrap_or_default();
    let mut q = held;
    q.extend(items);
    q.extend(cont);
    r.set("ended", v.int(1), true);
    r.set("inspen", v.int(0), true);
    let fin = r.ops.fin;
    r.kid(fin, vec![v.list(q)]);
    // the `}` was the routine's last token: nothing follows it
    r.frames.clear();
    r.eof = true;
    Ok(())
}

/// `Dvi`: the pages, written (§583–§645). The comment and `\mag` are
/// read at the end.
pub(crate) fn dvi(cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
    let v = cx.values();
    let raw = i64::from_ne_bytes(cx.node().raw().to_ne_bytes());
    let _ = cx.use_field(v.field(&format!("hint:{raw}")))?;
    let pages = cx.acc_range(v.field("pages"), None)?;
    let comment = cx.use_field(v.field("dvicomment"))?.map(|x| str_of(v, x)).unwrap_or_default();
    let mag = cx.use_field(v.field("param:mag"))?.map_or(1000, |x| int_of(v, x));
    let mut w = phitex_dvi::DviWriter::new(i32::try_from(mag).unwrap_or(1000), &comment);
    for page in v.seq_to_vec(pages) {
        let xs = list_of(v, page);
        let mut side = Side::default();
        let Some(Node::Box(bx)) = xs.first().and_then(|&b| nodes::decode(v, b, &mut side)) else {
            continue;
        };
        let mut counts = [0i32; 10];
        for (k, c) in xs.get(1).map(|&l| list_of(v, l)).unwrap_or_default().iter().enumerate().take(10) {
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
                Some(crate::fonts::bytes_of(&ys.get(1).map(|&s| str_of(v, s)).unwrap_or_default()))
            } else {
                None
            }
        };
        let _ = w.ship_out(&bx, counts, &fonts, &mut wh);
    }
    let bytes = w.finish();
    let s: String = bytes.iter().map(|&b| char::from(b)).collect();
    Ok(v.intern(Value::Str(s.into_boxed_str())))
}
