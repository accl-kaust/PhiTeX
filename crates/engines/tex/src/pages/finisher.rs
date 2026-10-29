//! Layer 5: one [`Finish`] node per page (docs/spec-layers.md §2; the
//! rules in `SPEC.md` §2.3, §3).
//!
//! A finisher is a child of the page loop, placed right after the piece
//! its page fired in and after the finishers of earlier pages fired in
//! the same piece, so its routine's definitions reach the text after the
//! page in TeX's order. Its arguments are its key and what the engine's
//! predictions need (`SPEC.md` §3.3) and nothing of the page's content:
//! the page is the loop node's definition [`super::PAGE_OF`]`:<piece>:<nth>`,
//! read at the unit's end ([`phitex_defuse::At::End`]). So a page whose
//! content moves re-asks its finisher; a finisher is made anew only when
//! a page appears or vanishes.
//!
//! The run: assemble the page from its [`Page`] alone (the pieces' items
//! through the `pitems` window, by path; the routines' material by
//! value), replay the builder on real nodes to the same fire ([`super::replay`]),
//! run the output routine on the piece's state at the candidate
//! ([`super::routine`], moved verbatim from the old glue), ship the page,
//! and publish: the routine's definitions at this position, the
//! give-back as the output (read by the loop through its placement) and
//! as [`PAGE_OUT`]`:<piece>:<nth>`, the next page's start for the text's
//! page-state reads ([`super::PAGE_START`]'s parts), [`super::GDEFS`],
//! the page key for the PDF side, [`super::FIN_DONE`].
//!
//! A stale finisher (a piece it names ran again since the loop folded it,
//! or its page's piece stands after it in the view) makes no page and
//! publishes nothing new (`Ctx::unchanged`), its output its last: the
//! loop re-fires the page on the new version and the finisher is
//! re-asked with the new page.

use phitex_defuse::{At, Ctx, Effect, MayDefine, Operation, Stop, ValueId, Values};

use super::*;

/// A page's give-back, defined by its finisher alone:
/// `pages:out:<piece>:<nth>` = `[piece, nth, [markers, list, \deadcycles]]`
/// (`SPEC.md` §3.2 "give-back"). The loop reads it through its placement
/// ([`phitex_defuse::Ctx::placed_output_or`]); the field is for the
/// text's tools and traces.
pub(crate) const PAGE_OUT: &str = "pages:out";

/// The finisher's arguments: `[piece, nth]`, the page's key and its
/// placement's identity, nothing else: an argument that changed between
/// the loop's runs would make the finisher anew. The routine's hint and
/// the page's class it predicts by (`SPEC.md` §3.3: by routine and page
/// class, never by page content) are the page's, which the run reads
/// ([`Page::hint`], [`Page::output_penalty`]) and the loop leaves for
/// the predictions in `Ops::fin_class` ([`fin_class_of`]).
pub(crate) struct FinishArgs {
    pub key: PageKey,
}

impl FinishArgs {
    pub(crate) fn encode(&self, v: &Values) -> Vec<ValueId> {
        vec![
            raw_value(v, Some(self.key.piece)),
            v.int(i64::from(self.key.nth)),
        ]
    }

    pub(crate) fn decode(v: &Values, args: &[ValueId]) -> Option<FinishArgs> {
        let [p, n] = args else { return None };
        Some(FinishArgs {
            key: PageKey {
                piece: raw_node(v, *p)?,
                nth: u32::try_from(v.as_int(*n)?).ok()?,
            },
        })
    }
}

/// The routine's hint and the page's class of the finisher with
/// arguments `args`, as the loop last fired its page (`Ops::fin_class`);
/// `(UNIT, ordinary)` where it has not.
pub(crate) fn fin_class_of(o: &crate::run::Ops, v: &Values, args: &[ValueId]) -> (ValueId, i32) {
    o.fin_class
        .locked()
        .get(&fin_page_key(v, args))
        .map_or((ValueId::UNIT, i32::MAX), |x| (x.0, x.1))
}

/// The page's index among the pages the loop fired, as it last fired the
/// page of a finisher with arguments `args` (`Ops::fin_class`).
pub(crate) fn fin_index_of(o: &crate::run::Ops, v: &Values, args: &[ValueId]) -> Option<i64> {
    o.fin_class
        .locked()
        .get(&fin_page_key(v, args))
        .map(|x| x.2)
}

pub(crate) struct Finish(pub Shared);

impl Operation for Finish {
    fn name(&self) -> &'static str {
        "finish"
    }

    /// The finisher's contributions (gdefs, pages:fin, the PDF side's),
    /// never `vprof`/`pitems`.
    fn contributes(&self, _: &[ValueId], v: &Values, f: Field) -> bool {
        !from_pieces(v, f)
    }

    /// The output routine's hint ([`crate::hints::routine_hint`]) and the
    /// fields a finisher defines itself; anything where the routine's
    /// text cannot be bounded, or the hint is not given.
    fn may_define(&self, args: &[ValueId], v: &Values) -> MayDefine {
        let Some(a) = FinishArgs::decode(v, args) else {
            return MayDefine::Any;
        };
        let (t, pen) = fin_class_of(ops(&self.0), v, args);
        if t == ValueId::UNIT {
            return MayDefine::Any;
        }
        // (a finisher ran this routine before: what it defined then,
        // by name, and the fields a finisher sets itself; a definition
        // outside them refutes the declaration, as any node's does, and
        // only its readers run again)
        let known = ops(&self.0)
            .fin_defs
            .locked()
            .get(&fin_key_class(t, pen))
            .map(FinDefs::names);
        if let Some(mut names) = known {
            for n in FIN_OWN {
                names.push(v.field(n));
            }
            names.push(a.key.field(v, PAGE_OUT));
            return MayDefine::Names(names);
        }
        let h = crate::hints::Hint::decode(&str_of(v, t));
        let mut ps: Vec<String> = match h.may_define() {
            MayDefine::Any => return MayDefine::Any,
            MayDefine::Prefixes(ps) => ps,
            MayDefine::Names(ns) => ns.iter().map(|&f| crate::val::field_name(v, f)).collect(),
            MayDefine::Nothing => Vec::new(),
        };
        ps.extend(FINISHER_FIELDS.iter().map(|&s| s.to_owned()));
        ps.push(PAGE_OUT.to_owned());
        ps.push(RTN.to_owned());
        MayDefine::Prefixes(ps)
    }

    /// `SPEC.md` §3.3: what the last finisher running the same routine
    /// and page class defined, with its values (`FinDefs`, keyed by
    /// `(piece, nth)` for the page's own last run, else the routine's
    /// last two; never a page after it): the text after this one runs on
    /// them, and runs again only where a value differs. And its page's
    /// give-back ([`PAGE_OUT`]) as the routine's last, else nothing given
    /// back.
    fn predicted_defs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        let r = self.predicted_defs_(args, v);
        let c = if r.is_some() {
            &FIN_PREDICTED
        } else {
            &FIN_UNPREDICTED
        };
        c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut out = r.unwrap_or_default();
        let Some(a) = FinishArgs::decode(v, args) else {
            return Some(out);
        };
        let f = a.key.field(v, PAGE_OUT);
        let rtn = v.field(RTN);
        let given = out.iter().find(|e| e.0 == rtn).map_or_else(
            || v.list(vec![v.list(Vec::new()), v.list(Vec::new())]),
            |e| e.1,
        );
        out.retain(|e| e.0 != f);
        out.push((
            f,
            page_out_value(v, a.key.piece, a.key.nth, given, ValueId::UNIT),
        ));
        Some(out)
    }

    /// Page counters as relations: what the last finisher of the same
    /// routine and page class defined ([`FinDefs::rels`]: `\c@page` and
    /// the shipout count +1 where its page ships), and +0 on every
    /// register another class's run stepped. None before the class's
    /// first run.
    fn predicted_rels(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, i64)>> {
        Finish::predicted_rels_of(ops(&self.0), args, v)
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }

    /// The page layer settles at quiet points (docs/spec-settle.md): what
    /// a finisher defines re-asks the loop and the other finishers at
    /// once, and the text at the next quiet point.
    fn settles_at_quiet(&self) -> bool {
        true
    }

    /// 1. `args` = [`FinishArgs::decode`]; the page =
    ///    `read_at(PAGE_OF:<piece>:<nth>, At::End(unit))` decoded as
    ///    [`Page`]. Unset: the loop has not fired this page (the
    ///    placement is older than the loop's last commit): `unchanged`.
    /// 2. Stale (`SPEC.md` §3.1 "stale placement"): `unchanged`.
    /// 3. [`Run::assemble_page`], [`Run::replay_page`], the routine
    ///    (`fire_page`, `output_end`, `ship_out`), then
    ///    [`Run::publish_page`].
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let _busy = crate::exec::Busy::new(&ops(&self.0).watch, 3, cx.node().raw());
        let t_fin = std::time::Instant::now();
        let _ = phitex_pdf::phase::take();
        let o = ops(&self.0);
        #[allow(
            clippy::redundant_closure_for_method_calls,
            reason = "the method is not general enough over the run's lifetimes"
        )]
        let out = run_node(cx, o, |r| r.finish_page());
        if phitex_pdf::phase::on() {
            phitex_pdf::phase::add(0, t_fin);
            let a = phitex_pdf::phase::take();
            #[allow(clippy::cast_precision_loss)]
            let us: Vec<String> = a
                .iter()
                .map(|&n| format!("{:.1}", n as f64 / 1000.0))
                .collect();
            crate::exec::later(format!("finphase: {}", us.join(" ")));
        }
        out
    }
}

impl Run<'_, '_> {
    /// [`Finish::run`]'s steps 1–3, in its run's body ([`run_node`]).
    pub(crate) fn finish_page(&mut self) -> R<()> {
        let v = self.v;
        let args = self.cx.args().to_vec();
        let key = FinishArgs::decode(v, &args).map(|a| a.key);
        // 1. its page, the loop's definition read at the unit's end
        let mut page_x = None;
        let page = match key {
            Some(k) => match self
                .cx
                .read_at(k.field(v, PAGE_OF), At::End(self.cx.unit()))?
            {
                phitex_defuse::FieldAt::Def(x, _) => {
                    page_x = Some(x);
                    Page::decode(v, x).filter(|p| p.key == k)
                }
                _ => None,
            },
            None => None,
        };
        let nth = page.as_ref().map_or(0, |p| i64::from(p.nth));
        // (the progress shown: the page the build has reached)
        if let Some(p) = page.as_ref().filter(|p| p.normal > 0)
            && let Some(w) = self
                .ops
                .watch
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
        {
            w.progress.note_page(u64::try_from(p.normal).unwrap_or(0));
        }
        if crate::exec::obs_flag!("PHITEX_FIN_TRACE") {
            eprintln!(
                "fin-trace: node {} page {nth} of its piece spec {} page {}",
                self.cx.node().raw(),
                self.cx.is_speculative(),
                page.is_some()
            );
        }
        // 2. no page, or a stale one: what it published last stands
        let shipped = match &page {
            Some(p) => match self.assemble_page(p)? {
                Some(asm) => self.replay_page(p, &asm)?,
                None => None,
            },
            None => None,
        };
        self.ins_fold = None;
        let first = page.as_ref().is_some_and(|f| f.prev_fire.0 == -1);
        let Some(Shipped { args: fargs, brk }) = shipped else {
            self.cx.unchanged();
            self.fin = Some(FinCtx {
                nth,
                first,
                pending: Vec::new(),
                held_n: 0,
                default_out: false,
                rtn: None,
                args: Vec::new(),
                page,
                page_x,
                brk: None,
            });
            return Ok(());
        };
        // (a page fired at a forced special penalty, \outputpenalty at
        // -10001 or below: no layout decision, TeX's own serial chain from
        // the text to the routine and back (LaTeX's floats and marginpars
        // taking boxes from \@freelist and giving them back at -10002):
        // what the routine defines re-asks the text at once,
        // docs/spec-settle.md §2)
        if page
            .as_ref()
            .is_some_and(|p| p.output_penalty <= FORCED_SPECIAL)
        {
            self.cx.settle_now();
        }
        // 3. the routine on its page
        let a = |i: usize| fargs.get(i).copied().unwrap_or(ValueId::UNIT);
        self.fin = Some(FinCtx {
            nth: int_of(v, a(0)),
            first,
            pending: Vec::new(),
            held_n: 0,
            default_out: false,
            rtn: None,
            args: fargs.clone(),
            page,
            page_x,
            brk,
        });
        self.preload_candidate(&fargs)?;
        if let Some(names) = crate::exec::obs_str!("PHITEX_FIN_SHOW") {
            let mut shown = Vec::new();
            for n in names.split(',') {
                let x = self.get(n)?;
                shown.push(format!(
                    "{n}={}",
                    x.map_or_else(|| "undefined".to_owned(), |x| crate::val::show_def(v, x))
                ));
            }
            let locals: Vec<String> = list_of(v, a(3))
                .iter()
                .filter_map(|&e| list_of(v, e).first().map(|&f| str_of(v, f)))
                .collect();
            eprintln!(
                "finisher {} page {} (piece {}, candidate {}, locals {:?}, args {}): {}",
                self.cx.node().raw(),
                int_of(v, a(0)),
                v.render(a(5)),
                v.render(a(6)),
                locals,
                fargs.len(),
                shown.join(", ")
            );
        }
        self.fire_page(a(1))?;
        self.run_all()?;
        self.publish_page()
    }

    /// `SPEC.md` §3.2 "give-back", "globals", "where the next page
    /// begins": what the routine leaves, at this position. (Its give-back
    /// as [`PAGE_OUT`] and the run's output, and [`FIN_DONE`], are the
    /// run's end's, `Run::finish`.)
    pub(crate) fn publish_page(&mut self) -> R<()> {
        // (no `vprof` nor `pitems`, not even empty: the operation declares
        // it contributes to neither, and a contribution the loop's next
        // read met re-asked it at every finisher's first commit)
        let args = self
            .fin
            .as_ref()
            .map(|c| c.args.clone())
            .unwrap_or_default();
        self.give_way(&args)?;
        let g = self.contribute_global_defs();
        self.record_finisher_defs(&args, g);
        self.define_page_start()
    }
}

/// Finishers created with a prediction of their definitions, and
/// without (reported with `PHITEX_TEX_TIME`).
pub(crate) static FIN_PREDICTED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub(crate) static FIN_UNPREDICTED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// A page's last finisher run in [`FinDefs::pages`]: its checkpoint
/// index, its definitions, the page number it read.
pub(crate) type FinPage = (i64, Vec<(Field, ValueId)>, Option<i64>);

/// What the last two finishers running one routine defined
/// (`Ops::fin_defs`): the earlier run's definitions, then the last's.
#[derive(Clone, Debug, Default)]
pub(crate) struct FinDefs {
    pub prev: Vec<(Field, ValueId)>,
    pub last: Vec<(Field, ValueId)>,
    /// The pages of the runs `prev` and `last` were.
    pub prev_j: i64,
    pub last_j: i64,
    /// Every field a run of the routine defined so far: a routine
    /// defines some on some pages only (a float page, a page with a
    /// list continued), not on the last two.
    pub seen: Vec<Field>,
    /// Per field a run defined: the value the latest such run (by page)
    /// left, its page, and whether the run before that defined it to
    /// the same value.
    pub recent: Vec<(Field, ValueId, i64, bool)>,
    /// What the last run contributed to `gdefs` (the fields it defined
    /// globally, [`Run::contribute_global_defs`]): the next finisher's
    /// declared contribution, a guess ([`Finish::static_contribs`]).
    pub gdefs: Option<ValueId>,
    /// Each page's last run (by [`fin_page_key`]): its ordinary pages
    /// fired and its definitions ([`FinDefs::at`]).
    pub pages: std::collections::BTreeMap<FinPageKey, FinPage>,
    /// The page number (`\count0` as it reached the routine) of the runs
    /// `prev` and `last` were, where they read it: the hole of a
    /// page-dependent macro's template ([`FinDefs::predict`]).
    pub prev_pno: Option<i64>,
    pub last_pno: Option<i64>,
    /// The relations the last run defined (an integer register's global
    /// `\\advance`, `crate::regfold`: `\\c@page` stepped as the page
    /// ships): the next finisher's predicted relations
    /// ([`Finish::predicted_rels`]).
    pub rels: Option<Vec<(Field, i64)>>,
}

impl FinDefs {
    /// Every value it holds, for a collection ([`crate::collect`]).
    pub(crate) fn roots(&self, keep: &mut dyn FnMut(ValueId)) {
        let defs = self
            .prev
            .iter()
            .chain(&self.last)
            .chain(self.pages.values().flat_map(|p| &p.1));
        for &(f, x) in defs {
            keep(f.0);
            keep(x);
        }

        for &(f, x, _, _) in &self.recent {
            keep(f.0);
            keep(x);
        }
        self.gdefs.into_iter().for_each(&mut *keep);
    }

    /// The history a finisher of page `idx` predicts from: the page's
    /// own last run where it ran before (a finisher placed again, a round
    /// after the first), both runs that one; else the last two runs. (A
    /// page's key is where it stands, no page count: what the routine
    /// leaves that steps by page, a page counter, is a relation,
    /// [`Finish::predicted_rels`].)
    pub(crate) fn at(&self, idx: FinPageKey) -> FinDefs {
        let mut d = FinDefs {
            recent: self.recent.clone(),
            rels: self.rels.clone(),
            ..FinDefs::default()
        };
        if let Some((j, defs, pno)) = self.pages.get(&idx) {
            d.last.clone_from(defs);
            d.prev.clone_from(defs);
            (d.last_j, d.prev_j) = (*j, *j);
            (d.last_pno, d.prev_pno) = (*pno, *pno);
            return d;
        }
        d.last.clone_from(&self.last);
        d.prev.clone_from(&self.prev);
        (d.last_j, d.prev_j) = (self.last_j, self.prev_j);
        (d.last_pno, d.prev_pno) = (self.last_pno, self.prev_pno);
        d
    }

    /// The fields any run of the routine defined.
    pub(crate) fn names(&self) -> Vec<Field> {
        self.seen.clone()
    }

    /// The definitions both runs made with the same value: stable
    /// across pages, so a reader may run on them; a page number or a
    /// mark, changing every page, is not predicted (a wrong prediction
    /// runs its readers again).
    pub(crate) fn stable(&self, v: &Values) -> Vec<(Field, ValueId)> {
        self.last
            .iter()
            .filter(|d| {
                self.prev.contains(d)
                    && predictable(v, &crate::val::field_name(v, d.0), d.1, Some(d.1))
            })
            .copied()
            .collect()
    }

    /// [`FinDefs::stable`] for the finisher of page `j`, with the
    /// integers that step uniformly by page (`\count0`, a page
    /// counter) carried on to page `j` (spec-predicted-breaks §3).
    /// (`j`: the ordinary pages fired up to the page, where known; a page
    /// counter is carried on to it only then.)
    pub(crate) fn predict(
        &self,
        v: &Values,
        j: Option<i64>,
        page: Option<i64>,
    ) -> Vec<(Field, ValueId)> {
        // (the page number of page `j`: the last run's, stepped as its
        // relation on `\count0` steps it per page; a macro holding the
        // last run's page number is predicted with this one in its place,
        // spec-state-model §2.4)
        let c0 = v.field("count:0");
        let step = self
            .rels
            .as_ref()
            .and_then(|rs| rs.iter().find(|r| r.0 == c0).map(|r| r.1));
        // (only where the last two runs agree on it, their page numbers
        // one step a page apart: a run on an early guess read another)
        let agree = match (self.prev_pno, self.last_pno, step) {
            (Some(a), Some(b), Some(c)) => c
                .checked_mul(self.last_j - self.prev_j)
                .is_some_and(|d| a.checked_add(d) == Some(b)),
            _ => false,
        };
        let pno = match (self.last_pno, page, step) {
            (Some(p), Some(j), Some(c)) if agree => c
                .checked_mul(j - self.last_j)
                .and_then(|d| p.checked_add(d)),
            _ => None,
        };
        let template = |f: Field, x: ValueId| -> ValueId {
            match (self.last_pno, pno) {
                (Some(p), Some(q))
                    if p != q && v.with_str(f.0, |s| s.starts_with("cs:")).unwrap_or(false) =>
                {
                    substitute_page(v, x, p, q).unwrap_or(x)
                }
                _ => x,
            }
        };
        self.predict_values(v, j)
            .into_iter()
            .map(|(f, x)| (f, template(f, x)))
            .collect()
    }

    /// [`FinDefs::predict`] before its templates.
    fn predict_values(&self, v: &Values, j: Option<i64>) -> Vec<(Field, ValueId)> {
        let dj = self.last_j - self.prev_j;
        self.last
            .iter()
            .filter_map(|&(f, x)| {
                // (the next finisher reads the outgoing state of this one
                // only as a prediction, spec-final §3.5: a value not
                // stable across pages the last run's, a guess that runs
                // its readers again where wrong; `\deadcycles` only as a
                // stable 0, [`predictable`], and no mark that changed,
                // [`varies_by_page`])
                let name = crate::val::field_name(v, f);
                let before = self.prev.iter().find(|d| d.0 == f).map(|d| d.1);
                if !predictable(v, &name, x, before) {
                    return None;
                }
                // (one run of the routine so far, spec-final §9b: its
                // values carried on, a page counter stepping by the pages
                // between; a wrong guess runs its readers again, a piece
                // never waits for a finisher)
                let Some(&(_, y)) = self.prev.iter().find(|d| d.0 == f) else {
                    if !name.starts_with("count:") {
                        return Some((f, x));
                    }
                    let b = v.as_int(x)?;
                    return Some((f, v.int(b.checked_add(j? - self.last_j)?)));
                };
                if x == y {
                    return Some((f, x));
                }
                // (a mark or the penalty the page fired at that differed
                // between the last two runs: the page's own, not guessed)
                if varies_by_page(&name) {
                    return None;
                }
                // (registers only: a page counter; not a hash, a penalty)
                if !name.starts_with("count:") {
                    return Some((f, x));
                }
                let (Some(a), Some(b)) = (v.as_int(y), v.as_int(x)) else {
                    return Some((f, x));
                };
                let Some(d) = b.checked_sub(a) else {
                    return Some((f, x));
                };
                if dj <= 0 || d % dj != 0 {
                    return Some((f, x));
                }
                let x = (d / dj).checked_mul(j? - self.last_j)?.checked_add(b)?;
                Some((f, v.int(x)))
            })
            .collect()
    }
}

/// `x`, a macro's meaning, with each number in its body that is page
/// number `from` as `\number` prints it (digits of category 12, no
/// digit next to them) made `to`; `None` where it holds none, or is no
/// macro ([`FinDefs::predict`]'s templates).
pub(crate) fn substitute_page(v: &Values, x: ValueId, from: i64, to: i64) -> Option<ValueId> {
    use crate::val::{Macro, Mean, Tok};
    let Mean::Macro(m) = Mean::decode(v, Some(x)) else {
        return None;
    };
    let digits = |n: i64| -> Vec<Tok> {
        n.to_string()
            .chars()
            .map(|c| Tok::ch(12, u32::from(c)))
            .collect()
    };
    let (fd, td) = (digits(from), digits(to));
    let is_digit =
        |t: &Tok| !t.is_cs_or_active() && t.cat() == Some(12) && (48..=57).contains(&t.chr());
    let b = &m.body;
    let mut out: Vec<Tok> = Vec::with_capacity(b.len() + 2);
    let mut hit = false;
    let mut i = 0;
    while i < b.len() {
        let here = b[i..].starts_with(&fd)
            && (i == 0 || !is_digit(&b[i - 1]))
            && b.get(i + fd.len()).is_none_or(|t| !is_digit(t));
        if here {
            out.extend(td.iter().copied());
            i += fd.len();
            hit = true;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    hit.then(|| {
        Mean::Macro(std::rc::Rc::new(Macro::new(
            m.flags,
            m.params.clone(),
            std::rc::Rc::from(out),
        )))
        .encode(v)
    })
}

/// A page's place among the pages a routine ran on ([`FinDefs::pages`]):
/// its piece and the pages fired in that piece before it ([`PageKey`]
/// as integers). No page's index among all the pages: none is an
/// argument.
pub(crate) type FinPageKey = (i64, i64);

/// [`FinPageKey`] of a finisher with arguments `args` ([`FinishArgs`]).
pub(crate) fn fin_page_key(v: &Values, args: &[ValueId]) -> FinPageKey {
    FinishArgs::decode(v, args).map_or((-1, 0), |a| {
        (
            i64::from_ne_bytes(a.key.piece.raw().to_ne_bytes()),
            i64::from(a.key.nth),
        )
    })
}

/// Whether a finisher's definition `x` of field `name`, which the run
/// before left as `y` (`None`: no run before), is predicted
/// ([`FinDefs::predict`]). `\deadcycles` only as 0 where both runs left
/// it 0 (both shipped their page, §1023): the fire steps it and a
/// `\shipout` clears it (§1012), so what a run leaves depends on the
/// page, and at `\end` it decides whether the page chain goes on
/// (§1054). Guessed as 1, as a page's run left it that shipped nothing,
/// a dead cycle predicted the next page fired by `\end`, whose finisher
/// predicted another: the chain grew without end on guesses while the
/// page it stood on was still to run. A guessed 0 ends a chain at most,
/// which the run, where wrong, starts again.
pub(crate) fn predictable(v: &Values, name: &str, x: ValueId, y: Option<ValueId>) -> bool {
    name != "deadcycles" || (y == Some(x) && v.as_int(x) == Some(0))
}

/// The fields a page's own items decide (the marks, §1012, and the
/// penalty the page fired at, §1013): predicted only where the last two
/// runs left them equal (a document without marks), never as the last
/// run's value where they differed ([`FinDefs::predict`]).
pub(crate) fn varies_by_page(name: &str) -> bool {
    name.starts_with("mark") || name == "param:outputpenalty"
}

/// What a finisher defines besides its routine's definitions
/// (`fire_page`, `output_end`, `ship_out`): field prefixes.
/// The fields every finisher defines itself, whatever its routine.
pub(crate) const FIN_OWN: [&str; 11] = [
    RTN,
    PAGE_START_ANCHOR,
    PAGE_START_FOLD,
    PAGE_START_PARAMS,
    PAGE_START_HEAD,
    "deadcycles",
    "param:outputpenalty",
    "inspen",
    "badness",
    "box:255",
    crate::pdfout::PAGE_KEYS,
];

pub(crate) const FINISHER_FIELDS: &[&str] = &[
    "deadcycles",
    PAGE_START,
    "mark:",
    "param:outputpenalty",
    "inspen",
    "box:",
    "badness",
    "write:",
    "pdf:",
];

/// The class of a page fired at `\outputpenalty` `p`: every ordinary
/// one (above -10001, §1013) alike, `i32::MAX`; a special one its own,
/// which its routine branches on (a float page, a cleared page): the
/// routine defines and gives back otherwise in each class, so each has
/// its own history ([`FinDefs`]).
pub(crate) fn pen_class(p: i32) -> i32 {
    if p > FORCED_SPECIAL { i32::MAX } else { p }
}

/// The highest special `\outputpenalty` (§1013's forced break below
/// -10000, `\penalty-10001` and lower): a page fired there is no layout
/// decision of the builder but the text calling its routine.
pub(crate) const FORCED_SPECIAL: i32 = -10001;

/// [`fin_key`] of routine `t` on a page of class `pen`.
pub(crate) fn fin_key_class(t: ValueId, pen: i32) -> u64 {
    t.raw()
        ^ u64::from(pen.cast_unsigned())
            .rotate_left(32)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

impl Finish {
    pub(crate) fn predicted_rels_of(
        o: &crate::run::Ops,
        args: &[ValueId],
        v: &Values,
    ) -> Option<Vec<(Field, i64)>> {
        if !o.fin_predict.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        let (t, pen) = fin_class_of(o, v, args);
        if t == ValueId::UNIT {
            return None;
        }
        let m = o.fin_defs.locked();
        let mut out = m.get(&fin_key_class(t, pen))?.rels.clone()?;
        for e in m.values() {
            for &(f, _) in e.rels.iter().flatten() {
                if !out.iter().any(|x| x.0 == f) {
                    out.push((f, 0));
                }
            }
        }
        Some(out)
    }

    /// What the last runs of the routine defined, carried to page `j`
    /// ([`FinDefs::predict`]), and always what the routine gives back to
    /// the main list ([`RTN`]): the last run's value, or nothing given
    /// back (the routine that ships its page, §1026). (The page builder
    /// guesses the same for itself, [`Run::guess_give_back`].)
    pub(crate) fn predicted_defs_(
        &self,
        args: &[ValueId],
        v: &Values,
    ) -> Option<Vec<(Field, ValueId)>> {
        if !ops(&self.0)
            .fin_predict
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return None;
        }
        let (t, pen) = fin_class_of(ops(&self.0), v, args);
        // (the ordinary pages fired up to it are its page's, which its
        // arguments do not carry: page counters are relations,
        // [`Finish::predicted_rels`])
        let j = None;
        let o = ops(&self.0);
        let page = fin_index_of(o, v, args);
        let m = o.fin_defs.locked();
        let idx = fin_page_key(v, args);
        let d = (t != ValueId::UNIT)
            .then(|| m.get(&fin_key_class(t, pen)))
            .flatten()
            .map(|d| d.at(idx));
        let d = d.as_ref();
        let mut out = d
            .map(|d| {
                if false {
                    d.stable(v)
                } else {
                    d.predict(v, j, page)
                }
            })
            .unwrap_or_default();
        out.retain(|e| !transient(v, e.0));
        // (`PHITEX_PRED_SHOW=1`, observe: whether a finisher's
        // prediction has the page parameters)
        if crate::exec::obs_flag!("PHITEX_PRED_SHOW") {
            let pp = v.field("param:vsize");
            eprintln!(
                "pred-show: {} defs, vsize {}",
                out.len(),
                out.iter().any(|e| e.0 == pp)
            );
        }
        let rtn = v.field(RTN);
        // (the last run's give-back; none where the routine never ran:
        // a guess of nothing given back is wrong for every routine that
        // writes a register, and a finisher whose run defines otherwise
        // than its text predicts waits for what it guessed on, in a cold
        // build the page builder, to the end)
        if !out.iter().any(|e| e.0 == rtn)
            && let Some(last) = d.and_then(|d| d.last.iter().find(|e| e.0 == rtn).map(|e| e.1))
        {
            out.push((rtn, last));
        }
        Some(out)
    }
}
impl Run<'_, '_> {
    /// `SPEC.md` §3.1: a page's items from its [`Page`] alone, then
    /// ([`Run::replay_page`]) the routine's input in full (`[nth, data,
    /// carried, locals, rest, piece, k, reads, defs, hint, normal,
    /// special, cuttable, start]`, [`FinCtx::args`]) and its page's break
    /// in its piece (`[piece, candidate, inside]`, [`FinCtx::brk`]):
    ///
    /// 1. the items its spans name: the [`PITEMS`] units of the window
    ///    `(fin.after, fin.at]` read by range, each piece's markers
    ///    resolved where the piece is ([`Run::resolve_params_of`]), a
    ///    routine's material from `fin.rtns`, a held-over insertion's
    ///    list from its split (§1021);
    /// 2. folded with the builder on real nodes from `fin.fold` and
    ///    `fin.params` ([`PageState::fold_items`]): the post-output build
    ///    first (`fin.post` spans: the material as it is, a piece's items
    ///    carried folded again with their builds and markers spent, then
    ///    `fin.post_markers` and the build at `fin.post_k`), then the
    ///    pieces' items to the fire: the page's data, `\box255` native
    ///    ([`Run::fin_box`]), the material carried;
    /// 3. where the candidate is inside the piece with commands after it
    ///    (not its last, [`PITEMS`]' `last`), the piece's names at that
    ///    candidate ([`Run::check_data`]) and its locals.
    ///
    /// A fire the real nodes do not make where the profiles made it is a
    /// defect: a panic in debug and observe builds, an engine diagnostic
    /// otherwise.
    #[allow(
        clippy::type_complexity,
        clippy::too_many_lines,
        clippy::assertions_on_constants,
        reason = "a long assembly; a debug or observe build stops on the defect"
    )]
    pub(crate) fn assemble_page(&mut self, fin: &Page) -> R<Option<Assembled>> {
        let me = |n: phitex_defuse::NodeId| i64::from_ne_bytes(n.raw().to_ne_bytes());
        let pid = me(fin.at);
        // (past the first page, the insertion classes' registers as the
        // builder has them where the page began, the routine before's
        // give-back among them: a class's marker in the window is those
        // with what the text set since, [`Run::insert_class_at`])
        self.ins_fold = (fin.prev_fire.0 >= 0).then(|| {
            (
                fin.prev_fire.0,
                fin.ins
                    .iter()
                    .map(|&m| crate::pages::class_of(self.v, m))
                    .collect(),
            )
        });
        // (the window's units, by piece: their items with the markers
        // resolved where each piece is)
        // (the window read through paths, [`Run::window_units`]: each
        // unit's piece, then only the units the page names, their items
        // whole and the own piece's other parts; a unit of the window
        // the page does not name changing (its items, its check) leaves
        // this finisher standing)
        let (pids, units, own_view) = self.window_units(fin, pid)?;
        let names_own = fin.spans.iter().any(
            |s| matches!(s.src, Src::Piece { piece, .. } | Src::Held { piece, .. } if piece == pid),
        );
        if names_own && !pids.contains(&pid) {
            // (the piece stands after this finisher in the view, placed
            // again, the page builder to place the finisher after it
            // again: a view the engine replaces, the window read here to
            // the piece re-asked when the piece comes before this node.
            // No page made on it, and none waited for: the piece may read
            // what this node defines)
            return Ok(None);
        }
        if self.stale_placement(fin, &pids)? {
            // (a piece the page names ran again since the page builder
            // folded it: the placement is stale, the builder's fold of
            // the new version places this finisher again; no page made
            // on it, the mismatch no defect)
            if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
                eprintln!(
                    "finisher {} (page {} of its piece): stale placement",
                    self.cx.node().raw(),
                    fin.key.nth
                );
            }
            return Ok(None);
        }
        if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
            eprintln!(
                "finisher {} (page {} of its piece) runs (piece {} candidate {}, after {:?}, {} spans, window {:?})",
                self.cx.node().raw(),
                fin.key.nth,
                fin.key.piece.raw(),
                fin.k,
                fin.after.map(phitex_defuse::NodeId::raw),
                fin.spans.len(),
                pids
            );
        }
        Ok(Some(Assembled {
            units,
            own: own_view,
        }))
    }

    /// The page's fire on the real nodes ([`Run::assemble_page`]'s
    /// items): the routine's input ([`Shipped`]); `None` where a span
    /// names an item its piece no longer has (a stale page).
    #[allow(
        clippy::too_many_lines,
        clippy::assertions_on_constants,
        reason = "a long replay; a debug or observe build stops on the defect"
    )]
    pub(crate) fn replay_page(&mut self, fin: &Page, asm: &Assembled) -> R<Option<Shipped>> {
        let v = self.v;
        let me = |n: phitex_defuse::NodeId| i64::from_ne_bytes(n.raw().to_ne_bytes());
        let pid = me(fin.at);
        let units = &asm.units;
        let own_view = asm.own;
        let mut unit_ix: phitex_defuse::FxHashMap<i64, usize> = phitex_defuse::FxHashMap::default();
        for (i, u) in units.iter().enumerate() {
            unit_ix.insert(u.0, i);
        }
        let defect = |r: &mut Self, what: String| {
            let msg = format!(
                "page {} of piece {} (finisher {}): {what}: the page builder over the profiles and the finisher's page part",
                fin.key.nth,
                fin.key.piece.raw(),
                r.cx.node().raw()
            );
            assert!(!cfg!(any(debug_assertions, feature = "observe")), "{msg}");
            r.engine_diag(&msg);
        };
        // (each page's given-back items decoded once, not per item)
        let rtns: std::cell::RefCell<phitex_defuse::FxHashMap<i64, std::rc::Rc<Vec<ValueId>>>> =
            std::cell::RefCell::default();
        let rtn_items = |page: i64| -> std::rc::Rc<Vec<ValueId>> {
            if let Some(x) = rtns.borrow().get(&page) {
                return x.clone();
            }
            let x = std::rc::Rc::new(
                fin.rtns
                    .iter()
                    .find(|r| r.0 == page)
                    .map(|r| {
                        let rs = list_of(v, r.1);
                        let mut out = rs.first().map(|&x| list_of(v, x)).unwrap_or_default();
                        out.extend(rs.get(1).map(|&x| list_of(v, x)).unwrap_or_default());
                        out
                    })
                    .unwrap_or_default(),
            );
            rtns.borrow_mut().insert(page, x.clone());
            x
        };
        let unit_of = |piece: i64| unit_ix.get(&piece).map(|&i| &units[i]);
        // (an item a source names, real)
        let item_raw = |src: Src| -> Option<ValueId> {
            match src {
                Src::Piece { piece, item } => {
                    let raw = &unit_of(piece)?.2;
                    match crate::pages::end_sub_of(item) {
                        None => raw.get(item as usize).copied(),
                        Some((e, s)) => list_of(v, *raw.get(e as usize)?)
                            .get(1 + s as usize)
                            .copied(),
                    }
                }
                Src::Rtn { page, item } => rtn_items(page).get(item as usize).copied(),
                Src::Held { piece, item, from } => {
                    let raw = &unit_of(piece)?.2;
                    let x = *raw.get(item as usize)?;
                    if from == 0 {
                        return Some(x);
                    }
                    let mut side = Side::default();
                    let Some(Node::Ins(mut ins)) = nodes::decode(v, x, &mut side) else {
                        return None;
                    };
                    // (§1021: the rest of a split, its top pruned)
                    let tail = ins.list.split_off((from as usize).min(ins.list.len()));
                    let (rem, _) =
                        phitex_typeset::page::prune_page_top(tail, ins.split_top_skip, false);
                    let b = phitex_typeset::pack::vpack(
                        rem.clone(),
                        Spec::Additional(0),
                        &PackParams::default(),
                    )
                    .node;
                    ins.height = b.height + b.depth;
                    ins.list = rem;
                    Some(nodes::encode(v, &Node::Ins(ins), &side))
                }
            }
        };
        // (§1013: a page before's best break, a penalty, is 10000 since)
        let item = |src: Src| -> Option<ValueId> {
            let x = item_raw(src)?;
            if fin.inf.contains(&src) && nodes::tag(v, x) == nodes::T_PENALTY {
                return Some(nodes::encode(
                    v,
                    &Node::Penalty(phitex_typeset::linebreak::INF_PENALTY),
                    &Side::default(),
                ));
            }
            Some(x)
        };
        let mut ps = PageState::fresh(0);
        ps.normal = fin.normal - i64::from(!fin.special);
        ps.fold = fin.fold;
        ps.params = fin.params;
        if crate::exec::obs_flag!("PHITEX_PAGE_TRACE") {
            eprintln!(
                "page-trace: finisher {} (piece {}, nth {}) begins with params {} and {} post spans of {}",
                self.cx.node().raw(),
                fin.key.piece.raw(),
                fin.key.nth,
                fin.params
                    .map_or_else(|| "none".to_owned(), |m| v.render(m)),
                fin.post,
                fin.spans.len()
            );
        }
        // (`\deadcycles` as the routine before left it, §1012: the page
        // builder's, never the finisher before's; the text's settings
        // after it come in order with the items, `DEAD_SLOT` markers)
        ps.dead = fin.prev_fire.1;
        let at = fin.at;
        let mut fire: Option<Fire> = None;
        let mut fired_at_end = false;
        // 1. the post-output build
        let post = (fin.post as usize).min(fin.spans.len());
        if post > 0 {
            for s in &fin.spans[..post] {
                for src in crate::pages::span_srcs(s) {
                    let Some(x) = item(src) else {
                        // (a span naming an item its piece no longer has:
                        // the piece ran again since the page builder folded
                        // it, past what the version check sees; the
                        // placement is stale, the builder places this
                        // finisher again on the new version)
                        if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
                            eprintln!(
                                "finisher {} (page {} of its piece): no item for {src:?}: stale placement",
                                self.cx.node().raw(),
                                fin.key.nth
                            );
                        }
                        return Ok(None);
                    };
                    let carried_raw = matches!(src, Src::Piece { item, .. } if crate::pages::end_sub_of(item).is_none())
                        && nodes::tag(v, x) != nodes::T_END;
                    if carried_raw {
                        match nodes::tag(v, x) {
                            nodes::T_BUILD_PAGE => ps.fold.since_bp = 0,
                            nodes::T_PAGE_PARAMS | nodes::T_SET_PAGE | nodes::T_INS_PARAMS => {}
                            nodes::T_ABORT => ps.pending.push(x),
                            _ => fold_item(v, 0, x, &mut ps.fold, &mut ps.pending),
                        }
                    } else {
                        ps.pending.push(x);
                    }
                }
            }
            ps.pending.extend(fin.post_markers.iter().copied());
            let bp = v.list(vec![
                v.int(nodes::T_BUILD_PAGE),
                v.int(i64::from(fin.post_k)),
            ]);
            let mut live = |c: u16| self.insert_class_at(c, at);
            fire = ps.fold_items(v, &[bp], &mut live)?;
            fired_at_end = post == fin.spans.len();
        }
        // 2. the pieces' items, piece by piece
        let mut i = post;
        while fire.is_none() && i < fin.spans.len() {
            let s = fin.spans[i];
            i += 1;
            let mut raw: Vec<ValueId> = Vec::new();
            for src in crate::pages::span_srcs(&s) {
                let Some(x) = item(src) else {
                    if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
                        eprintln!(
                            "finisher {} (page {} of its piece): no item for {src:?}: stale placement",
                            self.cx.node().raw(),
                            fin.key.nth
                        );
                    }
                    return Ok(None);
                };
                raw.push(x);
            }
            let p_at = match s.src {
                Src::Piece { piece, .. } => crate::pages::node_of(piece),
                _ => at,
            };
            let mut live = |c: u16| self.insert_class_at(c, p_at);
            fire = ps.fold_items(v, &raw, &mut live)?;
            fired_at_end = i == fin.spans.len() && fire.as_ref().is_some_and(|f| f.rest.is_empty());
        }
        // (the real nodes not where the profiles the loop folded put the
        // page: its pieces moved since the loop node fired it, and that
        // node, re-asked by them, is still to run: this run waits for its
        // commit, which fires the page again or keeps it, and runs then.
        // With the loop settled it is a defect of the page builder over
        // the profiles)
        let agrees = fire
            .as_ref()
            .is_some_and(|f| fired_at_end && f.k == fin.k && f.best == fin.best as usize);
        if !agrees && !self.cx.placer_settled() {
            if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
                eprintln!(
                    "finisher {} (page {} of its piece): ahead of its loop, waits for it",
                    self.cx.node().raw(),
                    fin.key.nth
                );
            }
            return Err(Halt::Stop(self.cx.wait_for_placer()));
        }
        let Some(mut fire) = fire else {
            defect(self, "no page fires on the real nodes".to_owned());
            return Ok(None);
        };
        if !fired_at_end || fire.k != fin.k || fire.best != fin.best as usize {
            defect(
                self,
                format!(
                    "the real nodes fire at candidate {} best {} (at the input's end {fired_at_end}), the profiles at {} best {}",
                    fire.k, fire.best, fin.k, fin.best
                ),
            );
        }
        self.fin_box = fire.b255.take();
        // (the piece the page fired in: its check where the candidate is
        // inside it, the rest of its items after the candidate)
        let own = units
            .iter()
            .find(|u| u.0 == pid)
            .map(|u| list_of(v, u.1))
            .unwrap_or_default();
        let a = |i: usize| own.get(i).copied().unwrap_or(ValueId::UNIT);
        let last = if own.is_empty() { -1 } else { int_of(v, a(1)) };
        let at_end = i64::from(fire.k) == last;
        let rest: Vec<ValueId> = if fin.in_material {
            // (the piece's items after its candidate, all to come)
            units
                .iter()
                .find(|u| u.0 == pid)
                .map(|u| {
                    let k = i64::from(fin.k);
                    u.2.iter()
                        .position(|&x| {
                            nodes::tag(v, x) == nodes::T_BUILD_PAGE && int_at(v, x, 1) == k
                        })
                        .map_or_else(Vec::new, |p| u.2[p + 1..].to_vec())
                })
                .unwrap_or_default()
        } else {
            std::mem::take(&mut fire.rest)
        };
        // (a page fired at a candidate inside its piece, the piece going
        // on: the piece's items after it are on the next page, read
        // there, [`PageStart::tail`], not here)
        let tail = !fin.in_material && !at_end && !own.is_empty();
        // (the own unit's other parts read as they are used: its locals
        // always, its check only where the candidate is inside it; of a
        // local or a name standing at the candidate, which it is, and its
        // value only where the routine uses it, [`Run::fin_lazy_value`])
        let part = |r: &mut Self, k: u32| {
            own_view
                .and_then(|(w, _)| r.cx.get(w, phitex_defuse::Step::Index(k)))
                .unwrap_or(ValueId::UNIT)
        };
        let locals = self.lazy_locals(own_view);
        let mut args = vec![
            v.int(i64::from(fin.nth)),
            fire.data,
            v.list(fire.carried.clone()),
            v.list(locals),
        ];
        let mut cuttable = false;
        if at_end || own.is_empty() {
            args.extend([ValueId::UNIT; 5]);
        } else {
            // (the names at the candidate, and whether the run could end
            // there)
            let defs_k;
            (defs_k, cuttable) = self.lazy_names_at(own_view, fire.k);
            let reads = ValueId::UNIT;
            if crate::exec::prof_flag!("PHITEX_FIN_ITEMS") {
                let d4 = part(self, 4);
                eprintln!(
                    "fin-parts: node {} piece {pid} k {} part3 {:016x} part4 {:016x} names {:016x} {}",
                    self.cx.node().raw(),
                    fire.k,
                    reads.raw(),
                    d4.raw(),
                    defs_k.raw(),
                    v.render(defs_k).chars().take(4000).collect::<String>()
                );
                eprintln!(
                    "fin-part3: node {} {}",
                    self.cx.node().raw(),
                    v.render(reads).chars().take(4000).collect::<String>()
                );
            }
            args.extend([
                v.list(rest.clone()),
                v.int(pid),
                v.int(i64::from(fire.k)),
                reads,
                defs_k,
            ]);
        }
        if crate::exec::prof_flag!("PHITEX_FIN_ITEMS") {
            eprintln!(
                "fin-locals: node {} {:016x} {}",
                self.cx.node().raw(),
                args[3].raw(),
                v.render(args[3]).chars().take(2000).collect::<String>()
            );
        }
        args.extend([fin.hint, v.int(fin.normal), v.int(i64::from(fin.special))]);
        args.resize(12, ValueId::UNIT);
        args.push(v.int(i64::from(cuttable)));
        // (where the next page begins, for [`Run::define_page_start`]:
        // the fold after the fire, the parameters in force, the rest)
        args.push(v.list(vec![
            v.int(i64::from(ps.fold.sf)),
            v.int(i64::from(ps.fold.pd)),
            v.int(i64::try_from(ps.fold.since_bp).unwrap_or(0)),
            ps.params.unwrap_or(ValueId::UNIT),
            v.list(if tail { Vec::new() } else { rest }),
            // (`\deadcycles` where the page fires, §1012)
            v.int(ps.dead),
            v.int(if tail { i64::from(fire.k) } else { -1 }),
            v.int(fin.after.map_or(-1, |n| i64::from_ne_bytes(n.raw().to_ne_bytes()))),
        ]));
        let brk = Some((pid, i64::from(fire.k), !at_end && !own.is_empty()));
        Ok(Some(Shipped { args, brk }))
    }

    /// Whether a piece finisher `fin`'s page names holds another version
    /// in the window (`pids`, the window's pieces) than the page builder
    /// folded ([`Page::vers`]): read as that comparison alone.
    fn stale_placement(&mut self, fin: &Page, pids: &[i64]) -> R<bool> {
        use phitex_defuse::Step;
        let v = self.v;
        if fin.vers.is_empty() {
            return Ok(false);
        }
        let w = self.view_through(PITEMS, fin.after, fin.at)?;
        for &(p, ver) in &fin.vers {
            let Some(i) = pids.iter().position(|&q| q == p) else {
                continue;
            };
            let same = self
                .cx
                .at(w, Step::Index(u32::try_from(i).unwrap_or(u32::MAX)))
                .and_then(|u| self.cx.at(u, Step::Index(8)))
                .and_then(|x| self.cx.get(x, Step::Eq(ver)));
            if same == Some(v.bool(false)) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The [`PITEMS`] window of finisher `fin` (after `fin.after`, to its
    /// piece `pid`) read through paths: every unit's piece (one read),
    /// then for each unit whose piece the page's spans name (or `pid`)
    /// its items, whole, with their markers resolved where the piece is,
    /// and for `pid` its last candidate (`[pid, last]` as the unit's
    /// shape) and its view, through which the finisher reads its other
    /// parts as it uses them. The finisher depends on those alone.
    #[allow(clippy::type_complexity)]
    pub(crate) fn window_units(
        &mut self,
        fin: &Page,
        pid: i64,
    ) -> R<(
        Vec<i64>,
        Vec<(i64, ValueId, Vec<ValueId>)>,
        Option<(phitex_defuse::View, u32)>,
    )> {
        use phitex_defuse::Step;
        let v = self.v;
        if cfg!(feature = "observe") {
            let k = PageNode::of(self.ops, v, self.cx.op(), self.cx.args());
            assert!(
                k.is_some_and(|k| k.folds().contains(&PITEMS)),
                "a window of {PITEMS} outside a finisher"
            );
        }
        let w = self.view_through(PITEMS, fin.after, fin.at)?;
        // (each unit's piece, read as that alone)
        let n = self.cx.len(w).unwrap_or(0);
        let mut pids: Vec<i64> = Vec::with_capacity(usize::try_from(n).unwrap_or(0));
        for i in 0..n {
            let i = u32::try_from(i).unwrap_or(u32::MAX);
            let p = self
                .cx
                .at(w, Step::Index(i))
                .and_then(|u| self.cx.get(u, Step::Index(0)))
                .map_or(i64::MIN, |p| int_of(v, p));
            pids.push(p);
        }
        let mut want: phitex_defuse::FxHashSet<i64> = phitex_defuse::FxHashSet::default();
        want.insert(pid);
        // (the items of each piece the page's spans name: a unit read at
        // those alone, so a change to a piece's items after the page's
        // cut (the next page's part of a piece that spans the break)
        // leaves this finisher standing; the own piece whole where the
        // page fired in the material before it, whose items after the
        // candidate all go on to the next page)
        for s in &fin.spans {
            if let Src::Piece { piece, .. } | Src::Held { piece, .. } = s.src {
                want.insert(piece);
            }
        }
        let named = named_items(fin);
        let whole = |p: i64| (p == pid && fin.in_material) || !named.contains_key(&p);
        let pick = |r: &mut Self, list: phitex_defuse::View, p: i64| r.items_at(list, &named[&p]);
        let mut units: Vec<(i64, ValueId, Vec<ValueId>)> = Vec::new();
        let mut own = None;
        // (a PDF page's lines read by their shape: their dimensions and
        // fonts, not their characters, `crate::pages::seal`;
        // `PHITEX_NO_SEAL=1` reads them whole)
        let seal = !crate::exec::obs_flag!("PHITEX_NO_SEAL") && self.param("pdfoutput")? > 0;
        // (the page parameters in force as the window's pieces come, from
        // those the page began with: a piece's `[tag]` marker is those and
        // what the text set since the page before fired,
        // docs/spec-settle.md §2.1)
        let mut cur = fin.params.map(|m| crate::pages::params_of(v, m));
        for (i, &p) in pids.iter().enumerate() {
            if !want.contains(&p) || units.iter().any(|u| u.0 == p) {
                continue;
            }
            let ui = u32::try_from(i).unwrap_or(u32::MAX);
            let Some(u) = self.cx.at(w, Step::Index(ui)) else {
                continue;
            };
            // (a PDF page's unit read by its shape, its items' view kept
            // for its sealed lines, `crate::pages::seal`)
            let shape = if seal {
                self.cx
                    .at(u, Step::Index(7))
                    .filter(|&w| self.cx.get(w, Step::Eq(ValueId::UNIT)) == Some(v.bool(false)))
            } else {
                None
            };
            let mut raw = match self.cx.at(u, Step::Index(2)) {
                Some(x) if let Some(sh) = shape => {
                    let raw_id =
                        |n: phitex_defuse::NodeId| i64::from_ne_bytes(n.raw().to_ne_bytes());
                    let place = u32::try_from(i).unwrap_or(u32::MAX);
                    self.seal_unit(p, x, place, fin.after.map(raw_id), raw_id(fin.at));
                    if whole(p) {
                        let sh = self.cx.whole(sh);
                        list_of(v, sh)
                    } else {
                        pick(self, sh, p)
                    }
                }
                Some(x) if whole(p) => {
                    let x = self.cx.whole(x);
                    list_of(v, x)
                }
                Some(x) => pick(self, x, p),
                None => Vec::new(),
            };
            // (`PHITEX_FIN_ITEMS=1`, profile: each unit's items a
            // finisher reads, one line each, to compare two evaluations)
            if crate::exec::prof_flag!("PHITEX_FIN_ITEMS") {
                for (k, &x) in raw.iter().enumerate() {
                    eprintln!(
                        "fin-items: node {} unit {p} item {k} {:016x} {}",
                        self.cx.node().raw(),
                        x.raw(),
                        v.render(x).chars().take(300).collect::<String>()
                    );
                }
            }
            // (a `[tag]` page parameter marker read where the piece is only
            // on the first page, before the builder has any in force: on
            // a later page it is the parameters the page began with or
            // the text set since, never a finisher's definition read by
            // its place, docs/spec-settle.md §2.1)
            let text = fin.params.is_some().then_some((fin.prev_fire.0, &mut cur));
            self.resolve_params_of(p, &mut raw, text)?;
            // (whether the page's candidate is the own piece's last, read
            // as that alone (a piece growing after the page's cut leaves
            // it standing): the candidate where so, else none (-2); its
            // other parts read later)
            let shape = if p == pid {
                own = Some((u, ui));
                let k = i64::from(fin.k);
                let at_last = self
                    .cx
                    .at(u, Step::Index(1))
                    .and_then(|w| self.cx.get(w, Step::Eq(v.int(k))))
                    == Some(v.bool(true));
                v.list(vec![v.int(p), v.int(if at_last { k } else { -2 })])
            } else {
                ValueId::UNIT
            };
            units.push((p, shape, raw));
        }
        Ok((pids, units, own))
    }

    /// `PHITEX_FIN_FIELD=name` (observe): the run's relation on that
    /// field beside its prediction, with the page's key, contributor,
    /// penalty and the class the prediction used.
    fn trace_fin_rel(&self, page: &Page, rels: &[(Field, i64)]) {
        let v = self.v;
        if let Some(name) = crate::exec::obs_str!("PHITEX_FIN_FIELD") {
            let f = v.field(name);
            let fa = self.cx.args().to_vec();
            let (t0, pen0) = fin_class_of(self.ops, v, &fa);
            eprintln!(
                "fin-rel node {} key {}:{} at {} pen {} (class guess {:?} {}): {name} {:?}, predicted {:?}",
                self.cx.node().raw(),
                page.key.piece.raw(),
                page.key.nth,
                page.at.raw(),
                page.output_penalty,
                t0 == page.hint,
                pen0,
                rels.iter().find(|r| r.0 == f).map(|r| r.1),
                Finish::predicted_rels_of(self.ops, &fa, v)
                    .and_then(|rs| rs.iter().find(|r| r.0 == f).map(|r| r.1))
            );
        }
    }

    /// A unit's items, or their shapes, at the indices `ixs` (sorted) of
    /// `list`, read as those alone ([`Run::window_units`]): the others
    /// unit, never read. A named index the list does not have ends the
    /// items there, as the list whole would (a miss, never a stand-in).
    fn items_at(&mut self, list: phitex_defuse::View, ixs: &[u32]) -> Vec<ValueId> {
        let n = ixs.last().map_or(0, |&i| i as usize + 1);
        let mut out = vec![ValueId::UNIT; n];
        for &i in ixs {
            let Some(x) = self.cx.get(list, phitex_defuse::Step::Index(i)) else {
                out.truncate(i as usize);
                break;
            };
            out[i as usize] = x;
        }
        out
    }
    /// The definitions this finisher publishes, kept for the next one
    /// running the same routine ([`Finish::predicted_defs`]).
    pub(crate) fn record_finisher_defs(&mut self, args: &[ValueId], gdefs: ValueId) {
        let v = self.v;
        let Some(&t) = args.get(9) else { return };
        // (a speculative run's too: a cold build's finishers all run on
        // predictions, and one seeding none leaves every later finisher
        // predicting nothing given back, its result unconfirmed until
        // the page builder ends; a wrong seed is a misprediction,
        // its readers run again)
        if t == ValueId::UNIT {
            return;
        }
        let mut out = Vec::new();
        let mut rels: Vec<(Field, i64)> = Vec::new();
        for (f, w, _) in self.publication() {
            match w {
                crate::state::Pub::Value(x) => {
                    let name = crate::val::field_name(v, f);
                    if is_page_field(&name) {
                        continue;
                    }
                    let x = self.unseal(x);
                    out.push((f, x));
                }
                crate::state::Pub::Rel(c) => rels.push((f, c)),
            }
        }
        if let Some(x) = self.fin.as_ref().and_then(|c| c.rtn) {
            out.push((v.field(RTN), x));
        }
        // (the page's index among the pages the loop fired, and its page
        // number as it reached the routine where the routine read it: the
        // templates' hole, [`FinDefs::predict`])
        let own = self.cx.args().to_vec();
        let j = fin_index_of(self.ops, v, &own).unwrap_or(0);
        let c0 = self.reg_loc(0, 0);
        let pno = self
            .st
            .engine_value(c0)
            .map(|x| i64::from(crate::val::i32_of(v, x)));
        // (`PHITEX_FIN_FIELD=name`: what each finisher's run left of that
        // field, for the predictions)
        if let Some(name) = crate::exec::obs_str!("PHITEX_FIN_FIELD") {
            let f = v.field(name);
            eprintln!(
                "fin-field page {j} node {}: {name} {}",
                self.cx.node().raw(),
                out.iter().find(|d| d.0 == f).map_or_else(
                    || "not defined".to_owned(),
                    |d| v.render(d.1).chars().take(80).collect()
                )
            );
        }
        let Some(page) = self.fin_page().cloned() else {
            return;
        };
        self.trace_fin_rel(&page, &rels);
        let mut m = self.ops.fin_defs.locked();
        let idx = (
            i64::from_ne_bytes(page.key.piece.raw().to_ne_bytes()),
            i64::from(page.key.nth),
        );
        let e = m
            .entry(fin_key_class(t, pen_class(page.output_penalty)))
            .or_default();
        e.gdefs = Some(gdefs);
        e.rels = Some(rels);
        for &(f, x) in &out {
            if !e.seen.contains(&f) {
                e.seen.push(f);
            }
            match e.recent.iter_mut().find(|r| r.0 == f) {
                Some(r) if j >= r.2 => *r = (f, x, j, r.1 == x),
                Some(_) => {}
                None => e.recent.push((f, x, j, true)),
            }
        }
        if crate::exec::obs_flag!("PHITEX_PRED_SHOW") {
            // (the page's class: the penalty it fired at, whether in a
            // routine's material, the dead cycles before it)
            let f = &page;
            let class = format!(
                "pen {} mat {} prev_dead {} classes {:?} nth {}",
                f.output_penalty, f.in_material, f.prev_fire.1, f.classes, f.key.nth
            );
            let r = |x: ValueId| v.render(x).chars().take(120).collect::<String>();
            let e = e.at(idx);
            for (f, x) in e.predict(v, None, Some(j)) {
                let got = out.iter().find(|d| d.0 == f).map(|d| d.1);
                if got != Some(x) {
                    let name = crate::val::field_name(v, f);
                    let prev = e.prev.iter().find(|d| d.0 == f).map(|d| d.1);
                    eprintln!(
                        "pred page {j} ({class}; last {} prev {}): {name} predicted {} got {} (last two {})",
                        e.last_j,
                        e.prev_j,
                        r(x),
                        got.map_or_else(|| "nothing".to_owned(), r),
                        if prev == e.last.iter().find(|d| d.0 == f).map(|d| d.1) {
                            "equal"
                        } else {
                            "differ"
                        }
                    );
                }
            }
        }
        e.pages.insert(idx, (j, out.clone(), pno));
        // (the two latest pages: finishers commit in any order; a page's
        // run again replaces its own entry, never makes the other: two
        // runs of one page agree on what changes from page to page, a
        // column kept, a mark, and would predict it for the next page)
        if j == e.last_j && !e.last.is_empty() {
            e.last = out;
            e.last_pno = pno;
        } else if j == e.prev_j && !e.prev.is_empty() {
            e.prev = out;
            e.prev_pno = pno;
        } else if j > e.last_j || e.last.is_empty() {
            e.prev = std::mem::take(&mut e.last);
            e.prev_j = e.last_j;
            e.prev_pno = e.last_pno;
            e.last = out;
            e.last_j = j;
            e.last_pno = pno;
        } else if j > e.prev_j || e.prev.is_empty() {
            e.prev = out;
            e.prev_j = j;
            e.prev_pno = pno;
        }
    }
}

/// The items of each piece finisher `fin`'s page names (its spans'
/// sources, sorted, once each): a unit of its window read at those alone
/// ([`Run::window_units`]).
fn named_items(fin: &Page) -> phitex_defuse::FxHashMap<i64, Vec<u32>> {
    let mut named: phitex_defuse::FxHashMap<i64, Vec<u32>> = phitex_defuse::FxHashMap::default();
    for s in &fin.spans {
        for src in crate::pages::span_srcs(s) {
            let (piece, ix) = match src {
                Src::Piece { piece, item } => (
                    piece,
                    crate::pages::end_sub_of(item).map_or(item, |(e, _)| e),
                ),
                Src::Held { piece, item, .. } => (piece, item),
                Src::Rtn { .. } => continue,
            };
            named.entry(piece).or_default().push(ix);
        }
    }
    for ixs in named.values_mut() {
        ixs.sort_unstable();
        ixs.dedup();
    }
    named
}
