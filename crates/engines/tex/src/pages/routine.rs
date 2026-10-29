//! The output routine's run (§1012–§1028, §638–§640, §1366–§1374;
//! `SPEC.md` §3.2): pdfTeX's semantics over the run machinery, moved
//! here verbatim from `finish.rs` at `bc394b811` (branch `latex-pages`)
//! with their docs: `preload_candidate`, `lazy_locals`, `lazy_names_at`,
//! `fin_lazy_value`, `fire_page` (the marks, insertion boxes,
//! `\outputpenalty`, `\deadcycles`, no routine / the routine's tokens),
//! `output_end` (the markers, `\box255` not void, `\insertpenalties`),
//! `ship_out`, `define_rtn`, `contribute_global_defs`, the globals for
//! the text after (`GDef`, `gdefs_after`, `replaced`), `transient`.
//! Their input is a [`super::Page`] and the assembled items
//! ([`super::Shipped`]'s arguments); nothing of the page's decoding,
//! window reads, stale checks or predictions lives here: those are the
//! finisher's (`finisher.rs`).

use super::{
    BEFORE_PIECE, Field, GDEFS, List, Locality, Mode, Node, PITEMS, PackParams, Page, Par, R, Run,
    Side, Spec, ValueId, Values, group, int_of, list_of, nodes, some, str_of, toks_of,
};

/// A routine's give-back as [`RTN`] holds it: `[markers, list]`, and
/// `[markers, list, dead]` where `\deadcycles` is not 0.
pub(crate) fn rtn_value(v: &Values, markers: ValueId, list: ValueId, dead: i64) -> ValueId {
    if dead == 0 {
        return v.list(vec![markers, list]);
    }
    v.list(vec![markers, list, v.int(dead)])
}

/// A give-back as a guess: `\deadcycles` 0 (a page shipped: a dead cycle
/// guessed would grow a chain of pages on guesses).
pub(crate) fn rtn_as_guess(v: &Values, x: ValueId) -> ValueId {
    let xs = list_of(v, x);
    if xs.len() < 3 {
        return x;
    }
    rtn_value(v, xs[0], xs[1], 0)
}

/// What a finisher's output routine gives back to the main list
/// (§1026): `[markers, list]`, the run's own main-list items (page
/// parameter markers) and the routine's list, without the held-over
/// insertions (the page builder has them); then, where not nothing,
/// `\deadcycles` as the routine left it (a page not shipped: §1054's
/// `\end` after it reads it) ([`rtn_value`]). The page builder folds the
/// next page on its guess of it ([`Run::guess_give_back`]) until the
/// finisher's output, this value ([`PAGE_OUT`]), says otherwise.
pub(crate) const RTN: &str = "rtn";

/// A finisher's page: its index and what the routine leaves for the
/// next page (§1026: the held-over insertions, then its list; the page
/// after it begins with them, with the material carried over and the
/// rest of the piece, [`Run::define_page_start`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct FinCtx {
    /// The pages fired in its page's piece before it ([`Page::nth`]).
    pub nth: i64,
    /// Whether its page is the first the page builder fired (no page
    /// fired before it: [`Page::prev_fire`]).
    pub first: bool,
    pub pending: Vec<ValueId>,
    /// How many of `pending` are the held-over insertions (§1022).
    pub held_n: usize,
    /// The page was shipped by the default output routine (§1023): the
    /// held-over insertions' count stays `\insertpenalties` (§1014), which
    /// a routine's end sets to 0 (§1026).
    pub default_out: bool,
    /// The `rtn` the finisher defined.
    pub rtn: Option<ValueId>,
    /// Its arguments in full ([`Run::finisher_args`]).
    pub args: Vec<ValueId>,
    /// Its page as the loop defined it ([`super::PAGE_OF`]), none where
    /// the loop has not fired it.
    pub page: Option<Page>,
    /// Its page's value as read (the [`super::PAGE_OF`] definition):
    /// with its give-back, its output ([`super::fin_output`]).
    pub page_x: Option<ValueId>,
    /// Its page's break in its piece (`[piece, candidate]`) and whether
    /// the piece goes on after it (the break is inside it).
    pub brk: Option<(i64, i64, bool)>,
}

// -------------------------------------------------------------- state

impl Run<'_, '_> {
    /// A finisher's end: what its routine gives back to the main list
    /// ([`RTN`]): its items, then the page parameters it set where no
    /// item of its own noted them, its list, and `\deadcycles` where not
    /// 0 (§1054's `\end` reads it) (§987: the build after the routine,
    /// §1026, reads them as the routine left them, not as they reached
    /// the piece the page fired in).
    pub(crate) fn define_rtn(&mut self, mut raw: Vec<ValueId>) -> R<()> {
        let v = self.v;
        if self.st.take_page_params() {
            raw.push(self.page_params_marker()?);
        }
        raw.extend(self.rtn_page_markers()?);
        let c = self.fin.clone().unwrap_or_default();
        let list = c.pending.get(c.held_n..).unwrap_or_default().to_vec();
        // (and `\deadcycles` as it leaves it where not 0, a page not
        // shipped: what `\end` reads after it, §1054)
        let dead = self.cint(crate::fields::cell::DEADCYCLES)?;
        let x = rtn_value(v, v.list(raw), v.list(list), dead);
        // (what it gives back leaves the run: its sealed lines' lists)
        let x = self.unseal(x);
        self.cx.define_with(v.field(RTN), x, Locality::Global);
        // (what the page builder folded the next page on, and a miss, it
        // counts where it reads this output, [`Feedback::RoutineGivesBack`])
        if let Some(c) = self.fin.as_mut() {
            c.rtn = Some(x);
        }
        Ok(())
    }

    /// What the build after the output (§1026) reads of the routine's end
    /// besides its list and the page parameters, as markers before the
    /// held-over insertions: `\insertpenalties` where the default output
    /// (§1023) leaves it the held-over insertions' count (§1014; a
    /// routine's end sets it to 0, §1026), and each held-over insertion's
    /// class registers as the fire and the routine left them (§1008 reads
    /// them as the class's first insertion meets the next page: `fire_up`
    /// filled the box, a routine may have emptied it), never where the
    /// page fired: the page builder and the text read them in the
    /// give-back's order, as the page parameters (docs/spec-settle.md
    /// §2.1).
    pub(crate) fn rtn_page_markers(&mut self) -> R<Vec<ValueId>> {
        let v = self.v;
        let Some(c) = self.fin.as_ref() else {
            return Ok(Vec::new());
        };
        let held = c.pending.get(..c.held_n).unwrap_or_default();
        let mut out = Vec::new();
        if c.default_out && !held.is_empty() {
            out.push(v.list(vec![
                v.int(nodes::T_SET_PAGE),
                v.int(crate::pages::INSPEN_SLOT),
                v.int(i64::try_from(held.len()).unwrap_or(0)),
            ]));
        }
        let mut ns: Vec<u16> = Vec::new();
        for &x in held {
            if nodes::tag(v, x) == nodes::T_INS {
                let n = u16::try_from(crate::val::int_at(v, x, 1)).unwrap_or(0);
                if !ns.contains(&n) {
                    ns.push(n);
                }
            }
        }
        // (and each class the page's fire filled (§1018) or the builder
        // had from a routine where the page began: their registers as
        // this routine left them reach the builder through its give-back
        // alone, never by the finisher's place, which the builder's
        // placements move (LaTeX's `\box\footins`, emptied by every
        // routine that ships a page with footnotes))
        if let Some(p) = c.page.as_ref() {
            for n in p.classes.iter().copied().chain(
                p.ins
                    .iter()
                    .filter_map(|&m| u16::try_from(crate::val::int_at(v, m, 1)).ok()),
            ) {
                if !ns.contains(&n) {
                    ns.push(n);
                }
            }
        }
        // (and each class whose registers the routine set (§1008 reads
        // them where the class's next insertion meets the page, not
        // where the routine ran): a box, `\count`, `\dimen` or `\skip`
        // under 255 this run defined)
        for n in 0..255 {
            if !ns.contains(&n) && self.ins_registers_defined(n) {
                ns.push(n);
            }
        }
        for n in ns {
            out.push(self.ins_params_marker(n)?);
        }
        Ok(out)
    }

    /// The pages fired in a finisher's page's piece before it (its first
    /// argument); -1 in any other run.
    /// A finisher's page as its step left it ([`FinCtx::pos`]; none
    /// outside a finisher's run).
    pub(crate) fn fin_page(&self) -> Option<&Page> {
        self.fin.as_ref().and_then(|c| c.page.as_ref())
    }

    pub(crate) fn page_nth(&self) -> i64 {
        self.fin.as_ref().map_or(-1, |c| c.nth)
    }

    /// Whether this run is the finisher of the first page fired.
    pub(crate) fn first_page(&self) -> bool {
        self.fin.as_ref().is_some_and(|c| c.first)
    }

    /// `fire_up` (§1012–§1025) for a finisher's page: the marks and the
    /// insertion boxes defined, `\deadcycles` counted, then the output
    /// routine pushed to run on this run's nest in its group (`true`), or
    /// the page shipped by the default routine (§1023; also after too
    /// many dead cycles, §1024), the held-over insertions pending.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn fire_page(&mut self, d: ValueId) -> R<bool> {
        let v = self.v;
        let data = list_of(v, d);
        let a = |i: usize| data.get(i).copied().unwrap_or(ValueId::UNIT);
        // (spec-final A4: `\box255` from the builder's native box, its
        // line boxes their own values: a box made once, here)
        let b255 = match self.fin_box.take() {
            Some((b, side)) => nodes::encode_box(v, &b, &side),
            None => a(0),
        };
        let (inserts, marks, held) = (list_of(v, a(1)), list_of(v, a(2)), list_of(v, a(3)));
        let output_penalty = data.get(4).map_or(0, |&x| int_of(v, x));
        if crate::exec::obs_flag!("PHITEX_FIRE_TEXT") {
            // (the page's first letters, to tell the pages apart)
            fn text(list: &[Node], out: &mut String) {
                for n in list {
                    if out.len() >= 60 {
                        return;
                    }
                    match n {
                        Node::Char { ch, .. } => {
                            out.push(char::from_u32(u32::from(*ch)).unwrap_or('?'));
                        }
                        Node::Lig(l) => {
                            for &c in &l.original {
                                out.push(char::from_u32(u32::from(c)).unwrap_or('?'));
                            }
                        }
                        Node::Box(b) => {
                            text(&b.list, out);
                            out.push(' ');
                        }
                        Node::Glue(_) => out.push(' '),
                        _ => {}
                    }
                }
            }
            let mut t = String::new();
            if let Some(Node::Box(b)) = nodes::decode(v, b255, &mut Side::default()) {
                text(&b.list, &mut t);
            }
            eprintln!(
                "fire-text: node {} page {} penalty {output_penalty} held {} spec {}: {t:?}",
                self.cx.node().raw(),
                self.fin.as_ref().map_or(-1, |c| c.nth),
                held.len(),
                self.cx.is_speculative()
            );
        }
        if let Some(c) = self.fin.as_mut() {
            c.held_n = held.len();
        }
        // §1012: the marks; those in force where the page began (every
        // class's `\botmark` before it) the page builder's, from its
        // page ([`Page::top_marks`]), not the finisher before's
        let before: Vec<(i64, ValueId)> = self
            .fin_page()
            .map(|f| f.top_marks)
            .filter(|&m| m != ValueId::UNIT)
            .map(|m| {
                list_of(v, m)
                    .into_iter()
                    .filter_map(|e| match list_of(v, e)[..] {
                        [c, m] => Some((int_of(v, c), m)),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let bot = before.iter().find(|e| e.0 == 0).map(|e| e.1);
        // (TeX's `top_mark:=bot_mark` where one is; none before the first)
        let top = bot;
        let mut first = None;
        let mut new_bot = bot;
        let classes: Vec<ValueId> = before
            .iter()
            .filter(|e| e.0 != 0)
            .map(|&(c, m)| v.list(vec![v.int(c), m, ValueId::UNIT, m]))
            .collect();
        crate::etex::fire_marks_values(self, &marks, Some(&classes))?;
        for &m in &marks {
            if crate::etex::is_class_mark(v, Some(m)) {
                continue;
            }
            if first.is_none() {
                first = Some(m);
            }
            new_bot = Some(m);
        }
        if first.is_none() {
            first = top;
        }
        for (name, m) in [
            ("mark:top", top),
            ("mark:first", first),
            ("mark:bot", new_bot),
        ] {
            self.set(name, m.unwrap_or(ValueId::UNIT), true);
        }
        // §1018–§1021: the insertion boxes
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
            let l = self.box_loc(n);
            self.set_cell(l, v.list(vec![v.int(0), b]), true);
        }
        self.set("param:outputpenalty", v.int(output_penalty), true);
        let output = self.par(Par::output)?;
        let mut toks = toks_of(v, output);
        // (`\deadcycles` where the page fired, as the page builder had it:
        // its arguments' ([`Run::finisher_args`]), not the finisher
        // before's definition)
        let dead = match self
            .fin
            .as_ref()
            .and_then(|c| c.args.get(13))
            .and_then(|&x| list_of(v, x).get(5).copied())
        {
            Some(x) => int_of(v, x),
            None => self.cint(crate::fields::cell::DEADCYCLES)?,
        };
        let max_dead = i64::from(self.param("maxdeadcycles")?);
        if !toks.is_empty() && dead >= max_dead {
            // §1024
            self.log(
                "error",
                &format!("Output loop---{dead} consecutive dead cycles"),
            )?;
            toks.clear();
        }
        if toks.is_empty() {
            // §1023: the default output routine: the held-over insertions
            // go back to the contributions, the page is shipped
            if let Some(c) = self.fin.as_mut() {
                c.pending = held;
                c.default_out = true;
            }
            self.ship_out(b255)?;
            return Ok(false);
        }
        // §1025
        self.set_cell(crate::fields::cell::DEADCYCLES, v.int(dead + 1), true);
        self.set_cell(
            crate::fields::cell::BOX255,
            v.list(vec![v.int(0), b255]),
            true,
        );
        self.push_list(-1, group::OUTPUT, vec![v.list(held)]);
        self.normal_paragraph()?;
        // (the routine's token list carries its braces, §1226: the group
        // is open already, so its `{` is not read; its `}` ends it)
        toks.remove(0);
        self.push_toks(toks);
        Ok(true)
    }

    /// The output group's `}` (§1026): the held-over insertions and the
    /// routine's list are what the next page sees first (folded at the
    /// run's end); `\box255` must be void; a page parameter the routine
    /// set governs the next page's breaks.
    pub(crate) fn output_end(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let (items, _) = self.list_items(md.lid)?;
        let mut left = md.extra.first().map(|&x| list_of(v, x)).unwrap_or_default();
        self.close_group(md)?;
        if self.st.take_page_params() {
            self.put_page_params();
        }
        if self.box_reg(255)?.is_some() {
            // §1028
            self.log("error", "Output routine didn't use all of \\box255")?;
            self.set_cell(
                crate::fields::cell::BOX255,
                v.list(vec![v.int(0), ValueId::UNIT]),
                true,
            );
        }
        self.set_cell(crate::fields::cell::INSPEN, v.int(0), true);
        left.extend(items);
        if let Some(c) = self.fin.as_mut() {
            c.pending = left;
        }
        Ok(())
    }

    /// `ship_out` (§638–§640): the page goes to the `pages` accumulator
    /// with its `\count0..9` and fonts; its `\write`s and `\openout`s
    /// happen now (§1366–§1374). It defines nothing for the text after
    /// (spec-final §3.2: no shipped mark).
    #[allow(clippy::too_many_lines)]
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
        let timed = crate::exec::prof_flag!("PHITEX_TEX_TIME");
        let t0 = std::time::Instant::now();
        let mut side = Side::default();
        // (the page's boxes are decoded once, here, for the writer: their
        // decodings are not kept for a later decode)
        // (its lines' lists as line breaking made them, where it kept
        // them, [`nodes::keep_made`])
        let prev = crate::val::NO_KEEP.with(|c| c.replace(true));
        let take = nodes::TAKE_MADE.with(|c| c.replace(true));
        let decoded = nodes::decode(v, b, &mut side);
        nodes::TAKE_MADE.with(|c| c.set(take));
        crate::val::NO_KEEP.with(|c| c.set(prev));
        if crate::exec::obs_flag!("PHITEX_MADE_STATS") {
            let n: Vec<u64> = nodes::MADE_STATS
                .iter()
                .map(|x| x.load(std::sync::atomic::Ordering::Relaxed))
                .collect();
            eprintln!(
                "made-stats: kept {} side {} full {} taken {} decoded {} lazy kept {} lazy not {}",
                n[0], n[1], n[2], n[3], n[4], n[5], n[6]
            );
        }
        let Some(Node::Box(bx)) = decoded else {
            return Ok(());
        };
        let t_decode = t0.elapsed();
        phitex_pdf::phase::add(1, t0);
        let t_p2 = std::time::Instant::now();
        // (the positions walk only for a whatsit that reads one: the
        // others act on their data; a page without one skips the walk,
        // the second copy of the box it makes and the font view)
        let needs_pos = side.whats.iter().any(|&w| {
            let xs = list_of(v, w);
            let kind = xs.first().map(|&k| str_of(v, k)).unwrap_or_default();
            crate::pdfprim::reads_position_at_shipout(&kind)
        });
        let pos = if needs_pos {
            crate::pdfprim::positions(self, &bx)?
        } else {
            crate::pdfprim::Positions {
                at: phitex_defuse::FxHashMap::default(),
                page_height: 0,
                pdf: self.param("pdfoutput")? > 0,
            }
        };
        let t_pos = t0.elapsed();
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
        let t_whats = t0.elapsed();
        phitex_pdf::phase::add(2, t_p2);
        let t_p3 = std::time::Instant::now();
        let page_box = Node::Box(bx);
        // (its sealed lines' fonts, from their stand-ins: their lists
        // are not read, `crate::pages::seal`)
        let sealed_fonts: Vec<u32> = side
            .lazy
            .iter()
            .filter_map(|&l| nodes::sealed_of(v, l))
            .flat_map(|s| list_of(v, s.fonts))
            .filter_map(|f| u32::try_from(int_of(v, f)).ok())
            .collect();
        let view = self.font_view_with(std::slice::from_ref(&page_box), &sealed_fonts)?;
        // (spec-final §3.2: the page's own font usage, by id; the end
        // node resolves names and files from the fonts' fields)
        let mut ids: Vec<u32> = view.fonts.ids();
        ids.extend(view.natives.keys().copied());
        ids.sort_unstable();
        let fonts: Vec<ValueId> = ids.iter().map(|&f| v.int(i64::from(f))).collect();
        let cs = v.list(counts.iter().map(|&c| v.int(i64::from(c))).collect());
        let pdf = self.pdf_page_params(pos.pdf)?;
        let mut parts = vec![b, cs, v.list(fonts), pdf, v.list(pdf_data)];
        // (a DVI's or XeTeX's page with sealed lines: where their units
        // are, for the end node that fills them in,
        // [`crate::assemble::fill_sealed_pages`]; a PDF page's writer
        // has them from this run)
        if !pos.pdf
            && side.lazy.iter().any(|&l| nodes::is_sealed(v, l))
            && let Some((after, to, at)) = self.sealed.places()
        {
            let mut w = vec![v.int(after.unwrap_or(-1)), v.int(to)];
            for (p, i) in at {
                w.extend([v.int(p), v.int(i64::from(i))]);
            }
            parts.push(v.list(w));
        }
        let page = v.list(parts);
        let t_fonts = t0.elapsed();
        phitex_pdf::phase::add(3, t_p3);
        let t_p4 = std::time::Instant::now();
        // (the page's hash: its value, what it is made of, known on the
        // write side every round; its PDF side runs once, at the end)
        // (the box handed over: the writer's piece keeps it, no copy)
        if pos.pdf
            && let Node::Box(bx) = page_box
        {
            crate::pdfout::ship(self, page, *bx, &side, &ids)?;
        }
        // (its content's hash: a page's identity across builds)
        let shown = Some(v.digest(page));
        phitex_pdf::phase::add(4, t_p4);
        if timed {
            crate::exec::later(format!(
                "shipout: decode_us {} positions_us {} whatsits_us {} fonts_us {} pdf_us {}",
                t_decode.as_micros(),
                t_pos.checked_sub(t_decode).unwrap().as_micros(),
                t_whats.checked_sub(t_pos).unwrap().as_micros(),
                t_fonts.checked_sub(t_whats).unwrap().as_micros(),
                t0.elapsed().checked_sub(t_fonts).unwrap().as_micros()
            ));
        }
        let su = v.seq_unit(page);
        self.contribute("pages", su);
        if let Some(shown) = shown {
            let h = v.seq_unit(v.int(i64::from_ne_bytes(shown.to_ne_bytes())));
            self.contribute("pagehash", h);
        }
        self.log("shipend", "]")?;
        self.set_cell(crate::fields::cell::DEADCYCLES, v.int(0), true);
        phitex_pdf::phase::add(15, t0);
        Ok(())
    }

    /// The parameters pdfTeX reads at each page (pdfTeX §750–§757), with
    /// whether the file is PDF and the colour stacks made so far.
    pub(crate) fn pdf_page_params(&mut self, pdf: bool) -> R<ValueId> {
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
            let f = self.ops.fields.param(v, p);
            let toks = toks_of(v, self.get_f(f)?);
            let mut s = String::new();
            self.show_toks_into(&mut s, &toks)?;
            xs.push(v.str(&s));
        }
        Ok(v.list(xs))
    }
}

/// A finisher's unit of [`GDEFS`]: the fields its routine defined
/// globally, and where its page fired, the piece and the candidate
/// ([`fin_at`]).
pub(crate) struct GDef {
    names: Vec<ValueId>,
    pid: i64,
    k: i64,
}

/// Where a finisher's page fired ([`Page::key`], [`Page::k`]):
/// the piece and its candidate; a page firing in the material the page
/// before left (the routine's, the material carried), the piece that page
/// fired in and a candidate after all of its own (`i64::MAX`).
pub(crate) fn fin_at(page: Option<&Page>) -> (i64, i64) {
    page.map_or((-2, i64::MAX), |f| {
        (
            i64::from_ne_bytes(f.at.raw().to_ne_bytes()),
            if f.in_material {
                i64::MAX
            } else {
                i64::from(f.k)
            },
        )
    })
}

/// A finisher's [`GDEFS`] unit: `[names, piece, candidate]`, nothing
/// where it defined nothing globally.
pub(crate) fn gdefs_unit(v: &Values, names: ValueId, (pid, k): (i64, i64)) -> ValueId {
    if names == ValueId::NIL {
        return ValueId::NIL;
    }
    v.seq_unit(v.list(vec![names, v.int(pid), v.int(k)]))
}

/// Whether a group-local binding of `f` the run of `piece` made after
/// its candidate `after` (-1: before its first, or before the run) is
/// replaced by a global definition of one of the routines `gdefs` (§279:
/// at every level): one of a page firing in another piece's text, or in
/// this piece's at a candidate the binding was made before.
pub(crate) fn replaced(gdefs: &[GDef], f: Field, piece: i64, after: i64) -> bool {
    gdefs
        .iter()
        .any(|g| g.names.contains(&f.0) && (g.pid != piece || after < g.k))
}

impl Run<'_, '_> {
    /// The [`GDEFS`] units of the finishers between node `pid` and this
    /// one.
    pub(crate) fn gdefs_after(&mut self, pid: i64) -> R<Vec<GDef>> {
        let v = self.v;
        let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(pid.to_ne_bytes()));
        let acc = self.fold_own(GDEFS, Some(at), None)?;
        Ok(v.seq_to_vec(acc)
            .into_iter()
            .filter_map(|e| match list_of(v, e)[..] {
                [n, p, k] => Some(GDef {
                    names: list_of(v, n),
                    pid: int_of(v, p),
                    k: int_of(v, k),
                }),
                _ => None,
            })
            .collect())
    }

    /// The last of the run's candidates passed before the definition
    /// with stamp `st` (-1: before the first, or none).
    pub(crate) fn made_after(&self, st: Option<u64>) -> i64 {
        let Some(st) = st else { return -1 };
        let n = self.cand_marks.iter().filter(|m| m.stamp < st).count();
        i64::try_from(n).unwrap_or(0) - 1
    }
}

impl Run<'_, '_> {
    /// A finisher's state where its page fired, before the routine
    /// runs: the bindings local to the groups open at the candidate
    /// (its fourth argument), and what the piece defined after the
    /// candidate as it was there, where the piece had it (its ninth): the
    /// routine runs at the candidate, before those definitions. One the
    /// piece had not read stays as it reaches here.
    pub(crate) fn preload_candidate(&mut self, args: &[ValueId]) -> R<()> {
        let v = self.v;
        let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
        let locals = list_of(v, a(3));
        if crate::exec::obs_flag!("PHITEX_PAGE_TRACE") {
            let names = |x: ValueId| {
                list_of(v, x)
                    .iter()
                    .filter_map(|&e| {
                        list_of(v, e)
                            .first()
                            .map(|&f| crate::val::field_name(v, Field(f)))
                    })
                    .filter(|n| n.starts_with("param:"))
                    .collect::<Vec<_>>()
                    .join(",")
            };
            eprintln!(
                "page-trace: finisher {} preloads locals [{}] names [{}]",
                self.cx.node().raw(),
                names(a(3)),
                if args.len() >= 9 {
                    names(a(8))
                } else {
                    String::new()
                }
            );
        }
        // (the routines of the pages firing in the piece before this one,
        // placed after it before this finisher: a binding they defined
        // globally is gone at every level where it was made before
        // their candidate, §279)
        let piece = self.fin.as_ref().and_then(|c| c.brk).map(|b| b.0);
        let gdefs = match piece {
            Some(p) if !locals.is_empty() => self.gdefs_after(p)?,
            _ => Vec::new(),
        };
        for e in locals {
            let ys = list_of(v, e);
            if let [f, x, ..] = ys[..] {
                let after = ys.get(2).map_or(-1, |&c| int_of(v, c));
                if replaced(&gdefs, Field(f), piece.unwrap_or(-2), after) {
                    continue;
                }
                // (a value read where the routine uses it, [`Run::fin_lazy_value`])
                let loc = self.loc_of(Field(f));
                if self.is_lazy(x) {
                    self.st.set_lazy(loc, Some(x));
                    continue;
                }
                self.st.preload(v, loc, some(x));
            }
        }
        if args.len() < 9 {
            return Ok(());
        }
        let piece =
            phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(int_of(v, a(5)).to_ne_bytes()));
        let before = v.str(BEFORE_PIECE);
        for e in list_of(v, a(8)) {
            let ys = list_of(v, e);
            let [f, _, kv] = ys[..] else { continue };
            if kv == ValueId::UNIT {
                continue;
            }
            let x = if kv == before {
                match self.field_at(Field(f), piece, true)? {
                    phitex_defuse::FieldAt::Def(x, _) => Some(x),
                    phitex_defuse::FieldAt::Unset => None,
                    phitex_defuse::FieldAt::Gone => continue,
                }
            } else if self.is_lazy(kv) {
                let loc = self.loc_of(Field(f));
                self.st.set_lazy(loc, Some(kv));
                continue;
            } else {
                list_of(v, kv).first().copied()
            };
            let loc = self.loc_of(Field(f));
            self.st.preload(v, loc, x);
        }
        Ok(())
    }

    /// The fields this finisher defined globally, for the resume after
    /// it ([`Run::cont_load`]): a group-local value the resume carries
    /// is replaced by a global definition even of an equal value.
    pub(crate) fn contribute_global_defs(&mut self) -> ValueId {
        let v = self.v;
        let names: Vec<ValueId> = self
            .publication()
            .into_iter()
            .filter(|d| d.2 == Locality::Global)
            .map(|d| d.0.0)
            .collect();
        // (none given as nothing, as the declared guess says it,
        // [`Finish::static_contribs`])
        let x = if names.is_empty() {
            ValueId::NIL
        } else {
            v.list(names)
        };
        if crate::exec::obs_flag!("PHITEX_FIN_DEFS") {
            let all: Vec<String> = self
                .publication()
                .into_iter()
                .map(|d| crate::val::field_name(v, d.0))
                .collect();
            let same: Vec<String> = Vec::new();
            eprintln!(
                "fin-defs node {} spec {}: {} published, {} global as read: {}  ALL: {}",
                self.cx.node().raw(),
                self.cx.is_speculative(),
                all.len(),
                same.len(),
                same.join(" "),
                all.join(" ")
            );
        }
        let at = fin_at(self.fin_page());
        self.contribute(GDEFS, gdefs_unit(v, x, at));
        x
    }
}

/// Whether field `f` is a command's transient state (§1266's
/// `after_token`): what a routine leaves there depends on what reached it
/// (it publishes only a change, `crate::runstate`), not on its page, so a
/// finisher declares and predicts none of it; a run that defines it
/// files it at its commit.
pub(crate) fn transient(v: &Values, f: Field) -> bool {
    f == v.field("aftertoken")
}

/// The tag of a value a finisher reads only where it uses it
/// ([`Run::fin_lazy_value`]): `[LAZY, unit, part, i]`, the own unit's
/// index in the window, its part (4 the check data, 5 the locals) and the
/// element.
const LAZY: &str = "\u{1}lazy";

impl Run<'_, '_> {
    /// The own piece's locals ([`Run::finisher_args`]): each `[field,
    /// value, after]`, the value a lazy tag: the run reads the fields and
    /// where they were made, a value only where the routine uses it.
    pub(crate) fn lazy_locals(&mut self, own: Option<(phitex_defuse::View, u32)>) -> Vec<ValueId> {
        use phitex_defuse::Step;
        let v = self.v;
        let Some((u, ui)) = own else {
            return Vec::new();
        };
        let Some(l) = self.cx.at(u, Step::Index(5)) else {
            return Vec::new();
        };
        // (each local's field and where it was made, read as those alone)
        let n = self.cx.len(l).unwrap_or(0);
        let mut fs = Vec::new();
        let mut afters = Vec::new();
        for i in 0..n {
            let Some(e) = self
                .cx
                .at(l, Step::Index(u32::try_from(i).unwrap_or(u32::MAX)))
            else {
                continue;
            };
            fs.push(self.cx.get(e, Step::Index(0)).unwrap_or(ValueId::UNIT));
            afters.push(self.cx.get(e, Step::Index(2)).unwrap_or_else(|| v.int(-1)));
        }
        let tag = v.str(LAZY);
        fs.iter()
            .enumerate()
            .map(|(j, &f)| {
                let lazy = v.list(vec![
                    tag,
                    v.int(i64::from(ui)),
                    v.int(5),
                    v.int(i64::try_from(j).unwrap_or(0)),
                ]);
                v.list(vec![
                    f,
                    lazy,
                    afters.get(j).copied().unwrap_or_else(|| v.int(-1)),
                ])
            })
            .collect()
    }

    /// The own piece's names at candidate `k` from its check data's index
    /// ([`super::piece::names_index_at`]): each `[field, standing, value]`, a
    /// value a lazy tag where the piece had one; and whether the run
    /// could end there.
    pub(crate) fn lazy_names_at(
        &mut self,
        own: Option<(phitex_defuse::View, u32)>,
        k: u32,
    ) -> (ValueId, bool) {
        use phitex_defuse::Step;
        let v = self.v;
        let Some((u, ui)) = own else {
            return (ValueId::UNIT, false);
        };
        let Some(d) = self.cx.at(u, Step::Index(4)) else {
            return (ValueId::UNIT, false);
        };
        let Some(cuts) = self.cx.at(d, Step::Index(1)) else {
            return (ValueId::UNIT, false);
        };
        let Some(cut) = self.cx.get(cuts, Step::Index(k)) else {
            return (ValueId::UNIT, false);
        };
        let index = self.cx.get(d, Step::Index(2)).unwrap_or(ValueId::UNIT);
        let tag = v.str(LAZY);
        let before = v.str(BEFORE_PIECE);
        let names = super::piece::names_index_at(v, index, k)
            .into_iter()
            .map(|(f, g, kind, i)| {
                let kv = match kind {
                    0 => ValueId::UNIT,
                    1 => before,
                    _ => v.list(vec![
                        tag,
                        v.int(i64::from(ui)),
                        v.int(4),
                        v.int(i64::try_from(i * 5 + 2).unwrap_or(0)),
                    ]),
                };
                v.list(vec![f, g, kv])
            })
            .collect();
        (v.list(names), int_of(v, cut) != 0)
    }

    /// The value a lazily preloaded field takes ([`Run::preload_candidate`]):
    /// a local's value as it is, a name's `[value]` or `[]`.
    pub(crate) fn fin_lazy_preload(&mut self, x: ValueId) -> R<Option<ValueId>> {
        let v = self.v;
        let part = list_of(v, x).get(2).map_or(0, |&p| int_of(v, p));
        let y = self.fin_lazy_value(x)?;
        Ok(if part == 5 {
            some(y)
        } else {
            list_of(v, y).first().copied()
        })
    }

    /// Whether `x` is a value a finisher reads only where it uses it.
    pub(crate) fn is_lazy(&self, x: ValueId) -> bool {
        let v = self.v;
        v.with_list(x, |xs| {
            xs.len() == 4 && v.with_str(xs[0], |s| s == LAZY).unwrap_or(false)
        })
        .unwrap_or(false)
    }

    /// The value lazy tag `x` stands for ([`Run::lazy_locals`],
    /// [`Run::lazy_names_at`]), read through the window now: the run
    /// depends on that element alone.
    pub(crate) fn fin_lazy_value(&mut self, x: ValueId) -> R<ValueId> {
        use phitex_defuse::Step;
        let v = self.v;
        let xs = list_of(v, x);
        let n =
            |i: usize| u32::try_from(xs.get(i).map_or(0, |&y| int_of(v, y))).unwrap_or(u32::MAX);
        let Some((after, piece)) = self.fin_page().map(|f| (f.after, f.at)) else {
            return Ok(ValueId::UNIT);
        };
        let w = self.view_through(PITEMS, after, piece)?;
        let Some(u) = self.cx.at(w, Step::Index(n(1))) else {
            return Ok(ValueId::UNIT);
        };
        let Some(p) = self.cx.at(u, Step::Index(n(2))) else {
            return Ok(ValueId::UNIT);
        };
        Ok(if n(2) == 5 {
            // (a local: `[field, value, after]`)
            self.cx
                .at(p, Step::Index(n(3)))
                .and_then(|e| self.cx.get(e, Step::Index(1)))
                .unwrap_or(ValueId::UNIT)
        } else {
            // (a name's value in the check data's stretches)
            self.cx
                .at(p, Step::Index(0))
                .and_then(|s| self.cx.get(s, Step::Index(n(3))))
                .unwrap_or(ValueId::UNIT)
        })
    }
}
