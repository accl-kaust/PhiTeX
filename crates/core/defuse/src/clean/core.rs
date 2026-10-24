//! What every world shares: the value table, the operations and their
//! static facts, projections, accumulators, loop-back declarations and the
//! environment.

use std::sync::{Arc, Mutex, RwLock};

use super::world::{Id, May, Static, Text, World};
use crate::api::{EvalError, Locality, Monoid, OpId, Operation, Projection};
use crate::values::{Field, FxHashMap, FxHashSet, Value, ValueId, Values, hash_words};

/// The environment the client supplies.
#[derive(Default)]
pub(crate) struct Env {
    pub known: FxHashMap<Field, ValueId>,
    pub absent: FxHashSet<Field>,
    /// The provider's answers (`None`: absent).
    pub provided: FxHashMap<Field, Option<ValueId>>,
    /// Fields the provider did not know, this evaluation.
    pub unknown: FxHashSet<Field>,
}

/// The first interned path's id (below it: 0 the whole value, else a
/// projection's id).
pub(crate) const PATH0: u32 = 1 << 16;

/// The `[Present]` path's id (interned first).
pub(crate) const PRESENT: u32 = PATH0;

/// The interned read paths, read from every worker without a lock: each
/// path's steps by id, and two memos keyed by hash (a hit checked, a miss
/// or a collision asked of the table under its lock): steps to their id,
/// and a path and a step after it to the path they make.
#[derive(Default)]
pub(crate) struct PathTable {
    steps: crate::sync::Table<Box<[crate::api::Step]>>,
    ids: crate::sync::Table<(Box<[crate::api::Step]>, u32)>,
    ext: crate::sync::Table<(u32, crate::api::Step, u32)>,
    /// Exact, for making paths (and for a hash collision).
    ix: Mutex<FxHashMap<Box<[crate::api::Step]>, u32>>,
}

/// A key for `sync::Table` (non-zero) from a hashable value.
fn table_key(x: &impl std::hash::Hash) -> u64 {
    use std::hash::Hasher;
    let mut h = crate::FxHasher::default();
    x.hash(&mut h);
    // (Fx leaves the low bits weak for small integers: mixed.)
    let k = h.finish().wrapping_mul(0x9e37_79b9_7f4a_7c15);
    k.max(1)
}

pub(crate) struct Core {
    pub values: Values,
    pub ops: Vec<Arc<dyn Operation>>,
    pub op_names: Vec<&'static str>,
    /// The most reads each operation's runs made lately (decaying): the
    /// size a run of it starts its read list at.
    pub op_reads: Vec<std::sync::atomic::AtomicU32>,
    pub projs: Vec<Projection>,
    /// The read paths interned (`intern_path`): id `PATH0 + i` is
    /// `list[i]`.
    pub paths: PathTable,
    /// While a run audit is on (`Engine::set_run_audit`): the reads the
    /// merge re-checked, by field (`None`: a read of no field), and how
    /// many it found broken.
    pub rechecks: Mutex<Option<FxHashMap<Option<Field>, (u64, u64)>>>,
    /// With them: the readers re-checked on all their reads, by why, and
    /// the reads asked (`RunAudit::rechecked_all`).
    pub rechecks_all: Mutex<FxHashMap<u8, (u64, u64)>>,
    /// Whether `rechecks` is on (read without its lock).
    pub auditing: std::sync::atomic::AtomicBool,
    /// `Engine::set_loop_option_check`.
    pub loop_check: std::sync::atomic::AtomicBool,
    /// `Step::Each` answers per sequence node and path.
    pub each_memo: crate::sync::Table<ValueId>,
    /// Nodes whose run broke at a read (its index, the first, and the
    /// world's log length then): one with marks resumes at the last before
    /// it (`Ctx::checkpoint`), its reads before it asked of the changes
    /// since.
    pub resume_at: Mutex<FxHashMap<u64, (u32, u64)>>,
    /// Runs cut off at a mark this evaluation.
    pub cut_in_run: std::sync::atomic::AtomicU64,
    pub cut_patched: std::sync::atomic::AtomicU64,
    /// Done nodes whose passed values were refiled this evaluation.
    pub passes_refiled: std::sync::atomic::AtomicU64,
    pub accs: FxHashMap<Field, Arc<dyn Monoid>>,
    /// The environment's base layer (`Engine::set_environment_base`) and
    /// its answers, each asked once for the engine's life.
    pub base: Option<Arc<crate::api::EnvBase>>,
    pub base_memo: crate::sync::Table<Option<ValueId>>,
    /// Accumulators declared by field-name prefix, first declared first,
    /// and each field's resolution among them (made on first use).
    pub acc_prefixes: Vec<(String, Arc<dyn Monoid>)>,
    pub acc_memo: crate::sync::Table<Option<Arc<dyn Monoid>>>,
    pub seqs: FxHashSet<Field>,
    pub stable: FxHashSet<Field>,
    /// Fields a reader guesses from its last evaluation's read of them
    /// while their definer is declared, not made (`Engine::guess_from_last`).
    pub guess_last: FxHashSet<Field>,
    /// Fields whose readers wait on a pending definition set that may hold
    /// them even when predicting (`Engine::wait_on_pending`).
    pub wait_pending: FxHashSet<Field>,
    /// Whether this evaluation's runs may guess (its scheduler speculates):
    /// a careful run that lapses guesses again.
    pub predicting: std::sync::atomic::AtomicBool,
    /// The siblings a pending claimer with no reach holds back
    /// (`Engine::set_claim_window`; `None`: all after it).
    pub claim_window: Option<usize>,
    /// With a claim window, a pending claimer is not held by the pending
    /// claimer before it (`Engine::set_claim_chain`).
    pub claim_chain: bool,
    /// The evaluation running (or last run): counted from 1 at each start.
    pub eval_no: std::sync::atomic::AtomicU64,

    /// Declared loop-back fields and their initial values.
    pub loops: FxHashMap<Field, ValueId>,
    /// Loop-backs declared by runs, taken in when the evaluation is quiet.
    /// Loop-back fields runs declared, each with its run's node.
    pub loops_later: Mutex<Vec<(Field, ValueId, u64, crate::api::LoopOptions)>>,
    /// The loop-back fields whose last publication in text order wins
    /// (`LoopOptions::last_wins`).
    pub last_wins: FxHashSet<Field>,
    pub env: RwLock<Env>,
    pub provider: Option<Box<crate::api::EnvProvider>>,
    /// Held while the provider is asked (runs asking for the same field at
    /// once ask it once).
    pub asking: Mutex<()>,
    /// Texts analysed, by `(op, args)` hash.
    pub texts: Mutex<FxHashMap<u64, Texts>>,
    /// Texts cached, and how many there were after the last pruning.
    pub texts_n: std::sync::atomic::AtomicUsize,
    pub texts_kept: std::sync::atomic::AtomicUsize,
    /// The first hook panic of the evaluation.
    pub hook_error: Mutex<Option<EvalError>>,
    pub text_memo: RwLock<FxHashMap<(ValueId, ValueId), ValueId>>,
    /// Operations whose nodes are marked at creation (`Engine::mark_op`).
    pub marked_ops: RwLock<FxHashSet<crate::api::OpId>>,
    /// The next unit id (shared by every world).
    pub next_unit: std::sync::atomic::AtomicU32,
    /// Each application's unit, by its node (the same in every world).
    pub app_units: Mutex<FxHashMap<u64, u32>>,
    /// Why each node was last made to run again (for the report).
    pub causes: Mutex<FxHashMap<u64, crate::api::Rerun>>,
    /// The provider's answers this evaluation.
    pub env_provided: std::sync::atomic::AtomicU64,
    /// Final nodes made dirty are held for the final phase: always, but
    /// from the previous values (`LoopStart::Previous`) only from an
    /// evaluation's second round on (in its first they run in the round,
    /// their result kept if no later round changes what they read).
    pub hold_fin: std::sync::atomic::AtomicBool,
}

impl Core {
    /// Field `f`'s accumulator: declared for it, or by the first declared
    /// prefix of its name (resolved once per field).
    pub(crate) fn acc(&self, f: Field) -> Option<&Arc<dyn Monoid>> {
        if let Some(m) = self.accs.get(&f) {
            return Some(m);
        }
        if self.acc_prefixes.is_empty() {
            return None;
        }
        self.acc_memo
            .get_or_insert(f.0.raw().max(1), || {
                let m = self
                    .values
                    .with_str(f.0, |name| self.acc_prefixes.iter().find(|(p, _)| name.starts_with(p.as_str())).map(|(_, m)| Arc::clone(m)))
                    .flatten();
                #[cfg(feature = "observe")]
                let m = m.map(|inner| -> Arc<dyn Monoid> { Arc::new(crate::record::RecMonoid { inner, field: f }) });
                m
            })
            .as_ref()
    }

    pub(crate) fn new() -> Core {
        let c = Core {
            values: Values::new(),
            ops: Vec::new(),
            op_names: Vec::new(),
            op_reads: Vec::new(),
            projs: Vec::new(),
            paths: PathTable::default(),
            rechecks: Mutex::new(None),
            rechecks_all: Mutex::new(FxHashMap::default()),
            auditing: std::sync::atomic::AtomicBool::new(false),
            loop_check: std::sync::atomic::AtomicBool::new(cfg!(debug_assertions) || cfg!(feature = "observe")),
            each_memo: crate::sync::Table::new(),
            resume_at: Mutex::new(FxHashMap::default()),
            cut_in_run: std::sync::atomic::AtomicU64::new(0),
            passes_refiled: std::sync::atomic::AtomicU64::new(0),
            cut_patched: std::sync::atomic::AtomicU64::new(0),
            accs: FxHashMap::default(),
            acc_prefixes: Vec::new(),
            acc_memo: crate::sync::Table::new(),
            base: None,
            base_memo: crate::sync::Table::new(),
            seqs: FxHashSet::default(),
            stable: FxHashSet::default(),
            guess_last: FxHashSet::default(),
            claim_window: None,
            claim_chain: false,
            wait_pending: FxHashSet::default(),
            predicting: std::sync::atomic::AtomicBool::new(false),
            eval_no: std::sync::atomic::AtomicU64::new(0),

            loops: FxHashMap::default(),
            loops_later: Mutex::new(Vec::new()),
            last_wins: FxHashSet::default(),
            env: RwLock::new(Env::default()),
            provider: None,
            texts: Mutex::new(FxHashMap::default()),
            texts_n: std::sync::atomic::AtomicUsize::new(0),
            texts_kept: std::sync::atomic::AtomicUsize::new(0),
            hook_error: Mutex::new(None),
            text_memo: RwLock::new(FxHashMap::default()),
            marked_ops: RwLock::new(FxHashSet::default()),
            next_unit: std::sync::atomic::AtomicU32::new(0),
            app_units: Mutex::new(FxHashMap::default()),
            causes: Mutex::new(FxHashMap::default()),
            env_provided: std::sync::atomic::AtomicU64::new(0),
            hold_fin: std::sync::atomic::AtomicBool::new(true),
            asking: Mutex::new(()),
        };
        // (`[Present]` first: `PRESENT`.)
        let p = c.intern_path(&[crate::api::Step::Present]);
        debug_assert_eq!(p, PRESENT);
        c
    }

    pub(crate) fn op_name(&self, op: OpId) -> &'static str {
        self.op_names.get(op.0 as usize).copied().unwrap_or("op")
    }

    /// Runs hook `hook` of `op`; a panic is the evaluation's error and the
    /// hook answers `dflt`.
    pub(crate) fn hook<T>(
        &self,
        who: Option<Id>,
        op: OpId,
        hook: &'static str,
        dflt: impl FnOnce() -> T,
        f: impl FnOnce() -> T,
    ) -> T {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            Ok(x) => x,
            Err(p) => {
                let message = panic_message(&*p);
                let e = EvalError::HookPanicked {
                    node: who.map(crate::api::NodeId),
                    op: self.op_name(op).to_string(),
                    hook,
                    message,
                };
                lock(&self.hook_error).get_or_insert(e);
                dflt()
            }
        }
    }

    /// The text `(op, args)`, analysed once.
    pub(crate) fn text(&self, who: Option<Id>, op: OpId, args: Arc<[ValueId]>) -> Arc<Text> {
        let h = text_hash(op, &args);
        {
            let t = lock(&self.texts);
            if let Some(v) = t.get(&h)
                && let Some(x) = v.iter().find(|x| x.op == op && x.args == args)
            {
                return x.clone();
            }
        }
        let stat = self.analyze(who, op, &args);
        let x = Arc::new(Text { op, args, stat });
        lock(&self.texts).entry(h).or_default().push(x.clone());
        self.texts_n.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        x
    }

    /// Drops the cached texts no node holds, once the cache has doubled
    /// since the last time (edits keep making texts; the cache stays
    /// within twice the live ones).
    pub(crate) fn prune_texts(&self) {
        use std::sync::atomic::Ordering::Relaxed;
        let n = self.texts_n.load(Relaxed);
        if n < 2 * self.texts_kept.load(Relaxed) + 4096 {
            return;
        }
        let mut t = lock(&self.texts);
        t.retain(|_, v| {
            v.retain(|x| Arc::strong_count(x) > 1);
            !v.is_empty()
        });
        let kept = t.values().map(Texts::len).sum();
        self.texts_n.store(kept, Relaxed);
        self.texts_kept.store(kept, Relaxed);
    }

    /// The text `(op, args)` analysed again (its hooks' answers may depend
    /// on context that changed): the cached analysis replaced.
    pub(crate) fn reanalyze(&self, op: OpId, args: Arc<[ValueId]>) -> Arc<Text> {
        let h = text_hash(op, &args);
        let stat = self.analyze(None, op, &args);
        let x = Arc::new(Text { op, args, stat });
        let mut t = lock(&self.texts);
        let v = t.entry(h).or_default();
        v.retain(|y| !(y.op == x.op && y.args == x.args));
        v.push(x.clone());
        x
    }

    fn analyze(&self, who: Option<Id>, op: OpId, args: &[ValueId]) -> Static {
        let o = &self.ops[op.0 as usize];
        let v = &self.values;
        let defs = self.hook(who, op, "defs", || None, || o.defs(args, v));
        let may: May = if defs.is_some() {
            May::Nothing
        } else {
            self.hook(
                who,
                op,
                "may_define",
                || crate::api::MayDefine::Any,
                || o.may_define(args, v),
            )
            .into()
        };
        let effect = match self.hook(who, op, "effect", || crate::api::Effect::Pending, || o.effect(args, v)) {
            crate::api::Effect::Pending => None,
            e => Some(e),
        };
        let contribs = self.hook(who, op, "static_contribs", || None, || o.static_contribs(args, v));
        let contribs = contribs.map(|mut cs| {
            // One per field (later ones combine as a run's would).
            let mut out: Vec<(Field, ValueId)> = Vec::new();
            for (f, x) in cs.drain(..) {
                if let Some(c) = out.iter_mut().find(|c| c.0 == f) {
                    c.1 = self.acc(f).map_or(x, |m| m.combine(v, c.1, x));
                } else {
                    out.push((f, x));
                }
            }
            Arc::from(out)
        });
        let preds: Option<Arc<[(Field, ValueId)]>> = self
            .hook(who, op, "predicted_defs", || None, || o.predicted_defs(args, v))
            .map(Arc::from);
        let fin = self.hook(who, op, "final_only", || false, || o.final_only(args, v));
        let contribs_complete = contribs.is_some() && self.hook(who, op, "contribs_complete", || false, || o.contribs_complete(args, v));
        let after_by_order = self.hook(who, op, "after_by_order", || false, || o.after_by_order(args, v));
        // (A pending set's predictions filed as entries: a reader after
        // the node takes its prediction instead of guessing past it.)
        let defs: Option<Arc<[(Field, Locality)]>> = defs.map(Arc::from);
        let filed = defs.clone().or_else(|| {
            let ps = preds.as_ref()?;
            let locs = self.hook(who, op, "predicted_localities", || None, || o.predicted_localities(args, v)).unwrap_or_default();
            let mut fs: Vec<(Field, Locality)> = Vec::with_capacity(ps.len());
            for &(f, _) in ps.iter() {
                if !fs.iter().any(|e| e.0 == f) {
                    let loc = locs.iter().find(|e| e.0 == f).map_or(Locality::Global, |e| e.1);
                    fs.push((f, loc));
                }
            }
            Some(Arc::from(fs))
        });
        Static {
            defs,
            filed,
            may,
            effect,
            contribs,
            contribs_complete,
            preds,
            fin,
            after_by_order,
            reads: self.hook(who, op, "reads", || None, || o.reads(args, v)).map(Arc::from),
        }
    }

    /// Whether a pending set `may` may cover `f`.
    pub(crate) fn may_cover(&self, may: &May, op: OpId, args: &[ValueId], f: Field) -> bool {
        let hit = match may {
            May::Nothing => false,
            May::Any => true,
            May::Names(ns) => ns.contains(&f),
            May::Prefixes(ps) => match &*self.values.get(f.0) {
                Value::Str(name) => ps.iter().any(|p| name.starts_with(p.as_str())),
                _ => false,
            },
        };
        hit && {
            let o = &self.ops[op.0 as usize];
            self.hook(None, op, "may_define_field", || true, || {
                o.may_define_field(args, &self.values, f)
            })
        }
    }

    /// Whether node text (`op`, `args`), a pending definer of `f`, likely
    /// defines it (`Operation::likely_defines`).
    pub(crate) fn likely_defines(&self, op: OpId, args: &[ValueId], f: Field) -> bool {
        let o = &self.ops[op.0 as usize];
        self.hook(None, op, "likely_defines", || true, || o.likely_defines(args, &self.values, f))
    }

    /// Path `steps`' id: 0 for none, a projection's own for one
    /// projection step, else interned (`PATH0` on).
    pub(crate) fn intern_path(&self, steps: &[crate::api::Step]) -> u32 {
        match steps {
            [] => 0,
            [crate::api::Step::Proj(p)] => u32::from(p.0),
            _ => {
                let t = &self.paths;
                let k = table_key(&steps);
                if let Some((s, i)) = t.ids.get(k)
                    && **s == *steps
                {
                    return *i;
                }
                let mut ix = lock(&t.ix);
                let i = match ix.get(steps) {
                    Some(&i) => i,
                    None => {
                        let i = PATH0 + u32::try_from(ix.len()).expect("too many paths");
                        ix.insert(steps.into(), i);
                        t.steps.get_or_insert(u64::from(i), || steps.into());
                        i
                    }
                };
                drop(ix);
                // (Taken by another path's steps: asked under the lock.)
                t.ids.get_or_insert(k, || (steps.into(), i));
                i
            }
        }
    }

    /// Path `p` with step `s` after it (memoized).
    pub(crate) fn extend_path(&self, p: u32, s: crate::api::Step) -> u32 {
        let k = table_key(&(p, s));
        if let Some(&(p2, s2, q)) = self.paths.ext.get(k)
            && p2 == p
            && s2 == s
        {
            return q;
        }
        let mut steps = self.path_steps(p).to_vec();
        steps.push(s);
        let q = self.intern_path(&steps);
        self.paths.ext.get_or_insert(k, || (p, s, q));
        q
    }

    /// Path `p`'s steps.
    pub(crate) fn path_steps(&self, p: u32) -> &[crate::api::Step] {
        if p == 0 {
            return &[];
        }
        if p < PATH0 {
            // (A projection's: its one step, kept under its id.)
            #[allow(clippy::cast_possible_truncation)]
            return self.paths.steps.get_or_insert(u64::from(p), || Box::new([crate::api::Step::Proj(crate::api::ProjId(p as u16))]));
        }
        self.paths.steps.get(u64::from(p)).map_or(&[], |s| &s[..])
    }

    /// Path `p`'s answer on value `x` (`None`: a step did not apply).
    pub(crate) fn eval_path(&self, p: u32, x: ValueId) -> Option<ValueId> {
        if p == 0 {
            return Some(x);
        }
        if p < PATH0 {
            #[allow(clippy::cast_possible_truncation)]
            return Some(self.project(p as u16, x));
        }
        let steps = self.path_steps(p);
        let mut cur = x;
        for &s in steps.iter() {
            cur = self.step(s, cur)?;
        }
        Some(cur)
    }

    /// `x` with its part at path `p` replaced by `new` (`None`: a step
    /// does not apply, or is not one a part is set by: an index of a list,
    /// a key of an association list).
    pub(crate) fn set_path(&self, p: u32, x: ValueId, new: ValueId) -> Option<ValueId> {
        let steps = self.path_steps(p);
        self.set_steps(steps, x, new)
    }

    fn set_steps(&self, steps: &[crate::api::Step], x: ValueId, new: ValueId) -> Option<ValueId> {
        use crate::api::Step;
        let Some((&s, rest)) = steps.split_first() else { return Some(new) };
        let v = &self.values;
        let items: Vec<ValueId> = v.with(x, |val| match val {
            Value::List(l) => Some(l.to_vec()),
            _ => None,
        })?;
        let mut items = items;
        match s {
            Step::Index(i) => {
                let e = items.get_mut(i as usize)?;
                *e = self.set_steps(rest, *e, new)?;
            }
            Step::Key(k) => {
                let i = items.iter().position(|&e| v.with(e, |ev| matches!(ev, Value::List(p) if p.len() >= 2 && p[0] == k)))?;
                let mut pair: Vec<ValueId> = v.with(items[i], |ev| match ev {
                    Value::List(p) => Some(p.to_vec()),
                    _ => None,
                })?;
                pair[1] = self.set_steps(rest, pair[1], new)?;
                items[i] = v.list(pair);
            }
            _ => return None,
        }
        Some(v.list(items))
    }

    /// One path step on `x`.
    pub(crate) fn step(&self, s: crate::api::Step, x: ValueId) -> Option<ValueId> {
        use crate::api::Step;
        let v = &self.values;
        let key = |k: ValueId| -> Option<ValueId> {
            v.with(x, |val| match val {
                Value::List(l) => l.iter().find_map(|&e| v.with(e, |ev| match ev {
                    Value::List(p) if p.len() >= 2 && p[0] == k => Some(p[1]),
                    _ => None,
                })),
                _ => None,
            })
        };
        match s {
            Step::Index(i) => v.with(x, |val| match val {
                Value::List(l) => l.get(i as usize).copied(),
                Value::Seq(_) => v.seq_nth(x, u64::from(i)),
                _ => None,
            }),
            Step::Last => v.with(x, |val| match val {
                Value::List(l) => l.last().copied(),
                Value::Seq(n) => v.seq_nth(x, n.len - 1),
                _ => None,
            }),
            #[allow(clippy::cast_possible_wrap)]
            Step::Len => v.with(x, |val| match val {
                Value::List(l) => Some(l.len() as i64),
                Value::Str(t) => Some(t.len() as i64),
                Value::Seq(n) => Some(n.len as i64),
                Value::Nil => Some(0),
                _ => None,
            })
            .map(|n| v.int(n)),
            Step::Key(k) => key(k),
            Step::Has(k) => Some(v.bool(key(k).is_some())),
            Step::Proj(p) => Some(self.project(p.0, x)),
            Step::Present => Some(v.bool(true)),
            Step::Eq(k) => Some(v.bool(x == k)),
            Step::Cmp(k) => match (v.as_int(x), v.as_int(k)) {
                (Some(a), Some(b)) => Some(v.int(a.cmp(&b) as i64)),
                _ => None,
            },
            Step::Indices(ix) => {
                let ix: Vec<ValueId> = v.with(ix, |val| match val {
                    Value::List(l) => l.to_vec(),
                    _ => Vec::new(),
                });
                let mut out = Vec::with_capacity(ix.len() + 1);
                out.push(ValueId::UNIT);
                for i in ix {
                    let Some(y) = v.as_int(i).and_then(|i| u32::try_from(i).ok()).and_then(|i| self.step(Step::Index(i), x)) else { break };
                    out.push(y);
                }
                #[allow(clippy::cast_possible_wrap)]
                let n = v.int((out.len() - 1) as i64);
                out[0] = n;
                Some(v.list(out))
            }
            Step::Keys(ks) => {
                let ks: Vec<ValueId> = v.with(ks, |val| match val {
                    Value::List(l) => l.to_vec(),
                    _ => Vec::new(),
                });
                let mut has = Vec::with_capacity(ks.len());
                let mut out = vec![ValueId::UNIT];
                for k in ks {
                    let y = key(k);
                    has.push(v.bool(y.is_some()));
                    out.extend(y);
                }
                out[0] = v.list(has);
                Some(v.list(out))
            }
            Step::Rank(k) => {
                let d = self.distinct(x)?;
                #[allow(clippy::cast_possible_wrap)]
                v.with(d, |val| match val {
                    Value::List(l) => l.iter().position(|&e| e == k).map(|i| i as i64 + 1),
                    _ => None,
                })
                .map(|n| v.int(n))
            }
            #[allow(clippy::cast_possible_wrap)]
            Step::Distinct => {
                let d = self.distinct(x)?;
                v.with(d, |val| match val {
                    Value::List(l) => Some(v.int(l.len() as i64)),
                    _ => None,
                })
            }
            Step::Ranks(ks) => {
                let d = self.distinct(x)?;
                let ks: Vec<ValueId> = v.with(ks, |val| match val {
                    Value::List(l) => l.to_vec(),
                    _ => Vec::new(),
                });
                #[allow(clippy::cast_possible_wrap)]
                let out = v.with(d, |val| match val {
                    Value::List(l) => ks.iter().map(|k| v.int(l.iter().position(|e| e == k).map_or(0, |i| i as i64 + 1))).collect(),
                    _ => Vec::new(),
                });
                Some(v.list(out))
            }
            Step::Each(p) => v
                .with(x, |val| match val {
                    Value::List(l) => Some(Err(l.to_vec())),
                    Value::Seq(_) | Value::Nil => Some(Ok(())),
                    _ => None,
                })
                .map(|k| match k {
                    Err(xs) => v.list(xs.into_iter().map(|e| self.eval_path(p.0, e).unwrap_or(ValueId::UNIT)).collect()),
                    Ok(()) => self.each_seq(p.0, x),
                }),
        }
    }

    /// The distinct elements of list or sequence `x` in the order they
    /// first occur, as a list (`Step::Rank`); a sequence's kept per rope
    /// node (made bottom up, without recursion): a node's are its left
    /// part's, then its right part's not among them.
    fn distinct(&self, x: ValueId) -> Option<ValueId> {
        let v = &self.values;
        let of = |l: &[ValueId]| {
            let mut out: Vec<ValueId> = Vec::new();
            for &e in l {
                if !out.contains(&e) {
                    out.push(e);
                }
            }
            v.list(out)
        };
        match v.with(x, |val| match val {
            Value::List(l) => Some(Ok(of(l))),
            Value::Nil => Some(Ok(v.list(Vec::new()))),
            Value::Seq(_) => Some(Err(())),
            _ => None,
        })? {
            Ok(d) => return Some(d),
            Err(()) => {}
        }
        let key = |x: ValueId| hash_words(0xd157, &[x.raw()]) | 1;
        let got = |x: ValueId| if x == ValueId::NIL { Some(v.list(Vec::new())) } else { self.each_memo.get(key(x)).copied() };
        let items = |d: ValueId| v.with(d, |val| match val {
            Value::List(l) => l.to_vec(),
            _ => Vec::new(),
        });
        let mut stack = vec![(x, false)];
        while let Some((y, expanded)) = stack.pop() {
            if got(y).is_some() {
                continue;
            }
            match v.seq_split(y) {
                Some(Ok(e)) => {
                    self.each_memo.get_or_insert(key(y), || v.list(vec![e]));
                }
                Some(Err((l, r))) if expanded => {
                    let (a, b) = (items(got(l).unwrap_or(ValueId::UNIT)), items(got(r).unwrap_or(ValueId::UNIT)));
                    let mut out = a;
                    for e in b {
                        if !out.contains(&e) {
                            out.push(e);
                        }
                    }
                    self.each_memo.get_or_insert(key(y), || v.list(out));
                }
                Some(Err((l, r))) => {
                    stack.push((y, true));
                    stack.push((r, false));
                    stack.push((l, false));
                }
                None => {}
            }
        }
        got(x)
    }

    /// `Step::Each(p)` on sequence `s`, kept per rope node (made bottom up,
    /// without recursion: a rope may be deep).
    fn each_seq(&self, p: u32, s: ValueId) -> ValueId {
        let v = &self.values;
        let key = |x: ValueId| hash_words(0xeac4, &[x.raw(), u64::from(p)]) | 1;
        let got = |x: ValueId| if x == ValueId::NIL { Some(ValueId::NIL) } else { self.each_memo.get(key(x)).copied() };
        let mut stack = vec![(s, false)];
        while let Some((x, expanded)) = stack.pop() {
            if got(x).is_some() {
                continue;
            }
            match v.seq_split(x) {
                Some(Ok(e)) => {
                    let a = self.eval_path(p, e).unwrap_or(ValueId::UNIT);
                    self.each_memo.get_or_insert(key(x), || v.seq_unit(a));
                }
                Some(Err((l, r))) if expanded => {
                    let (a, b) = (got(l).unwrap_or(ValueId::NIL), got(r).unwrap_or(ValueId::NIL));
                    self.each_memo.get_or_insert(key(x), || v.seq_concat(a, b));
                }
                Some(Err((l, r))) => {
                    stack.push((x, true));
                    stack.push((r, false));
                    stack.push((l, false));
                }
                None => {}
            }
        }
        got(s).unwrap_or(ValueId::NIL)
    }

    /// Path `p`'s steps for a recording: `Each` with its path's steps
    /// (path ids are this engine's).
    #[cfg(feature = "observe")]
    pub(crate) fn rec_steps(&self, p: u32) -> serde_json::Value {
        serde_json::Value::Array(self.path_steps(p).iter().map(|&s| self.rec_step(s)).collect())
    }

    /// One step for a recording (see `rec_steps`).
    #[cfg(feature = "observe")]
    pub(crate) fn rec_step(&self, s: crate::api::Step) -> serde_json::Value {
        match s {
            crate::api::Step::Each(q) => serde_json::json!({ "Each": self.rec_steps(q.0) }),
            s => crate::record::j(&s),
        }
    }

    pub(crate) fn project(&self, p: u16, x: ValueId) -> ValueId {
        let v = &self.values;
        if p == 1 {
            let empty = match &*v.get(x) {
                Value::Unit | Value::Nil => true,
                Value::Int(i) => *i == 0,
                Value::Str(s) => s.is_empty(),
                Value::List(l) => l.is_empty(),
                Value::Bool(_) | Value::Seq(_) => false,
            };
            return v.bool(empty);
        }
        match self.projs.get(usize::from(p).wrapping_sub(2)) {
            Some(pr) => pr(x, v),
            None => x,
        }
    }

    /// What an environment read of `f` finds: `Some(answer)`, or `None`
    /// unknown (the provider asked once per evaluation).
    #[allow(clippy::option_option)]
    /// What the environment answers for `f` without asking the provider:
    /// supplied, declared absent, the base layer's (asked once per field),
    /// or a provider answer already given.
    #[allow(clippy::option_option)]
    pub(crate) fn env_answer(&self, e: &Env, f: Field) -> Option<Option<ValueId>> {
        if let Some(&x) = e.known.get(&f) {
            return Some(Some(x));
        }
        if e.absent.contains(&f) {
            return Some(None);
        }
        if let Some(b) = &self.base
            && let Some(x) = *self.base_memo.get_or_insert(f.0.raw(), || b(&self.values, f))
        {
            return Some(Some(x));
        }
        e.provided.get(&f).copied()
    }

    pub(crate) fn env_get(&self, _w: &World, f: Field) -> Option<Option<ValueId>> {
        {
            let e = self.env.read().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(a) = self.env_answer(&e, f) {
                return Some(a);
            }
            if e.unknown.contains(&f) {
                return None;
            }
        }
        let p = self.provider.as_ref()?;
        let _asking = lock(&self.asking);
        {
            // (Answered while waiting for the turn.)
            let e = self.env.read().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(a) = self.env_answer(&e, f) {
                return Some(a);
            }
            if e.unknown.contains(&f) {
                return None;
            }
        }
        let Some(got) = p(&self.values, f) else {
            self.env
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .unknown
                .insert(f);
            return None;
        };
        self.env
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .provided
            .insert(f, got);
        self.env_provided.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Some(got)
    }

    /// What the environment holds for `f` without asking the provider:
    /// supplied, declared absent, or a provider answer already given.
    pub(crate) fn env_known(&self, f: Field) -> Option<Option<ValueId>> {
        self.env_answer(&self.env.read().unwrap_or_else(std::sync::PoisonError::into_inner), f)
    }

    /// Notes why node `id` runs again.
    pub(crate) fn cause(&self, id: u64, why: crate::api::Rerun) {
        lock(&self.causes).insert(id, why);
    }

    /// A new unit id.
    pub(crate) fn alloc_unit(&self) -> u32 {
        self.next_unit.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Application `app`'s unit (allocated once, then the same).
    pub(crate) fn app_unit(&self, app: u64) -> u32 {
        if let Some(&u) = lock(&self.app_units).get(&app) {
            return u;
        }
        let u = self.alloc_unit();
        *lock(&self.app_units).entry(app).or_insert(u)
    }

    pub(crate) fn is_loop(&self, f: Field) -> bool {
        self.loops.contains_key(&f)
    }

    pub(crate) fn declare_loop_later(&self, f: Field, init: ValueId, by: u64, o: crate::api::LoopOptions) {
        // (Checked, every declaration: a plain one may conflict too.)
        if o != crate::api::LoopOptions::default() || !self.loops.contains_key(&f) || self.loop_check.load(std::sync::atomic::Ordering::Relaxed) {
            lock(&self.loops_later).push((f, init, by, o));
        }
    }
}

/// The hash of a text.
pub(crate) fn text_hash(op: OpId, args: &[ValueId]) -> u64 {
    let mut w: Vec<u64> = Vec::with_capacity(args.len() + 1);
    w.push(u64::from(op.0));
    w.extend(args.iter().map(|a| a.raw()));
    hash_words(0x7e47, &w)
}

pub(crate) fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(non-string panic)".into())
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether `CLEAN_DEBUG` asks for traces (read once; `profile` builds
/// only: without the feature every trace compiles away).
#[inline]
pub(crate) fn debug() -> bool {
    #[cfg(feature = "profile")]
    {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("CLEAN_DEBUG").is_some())
    }
    #[cfg(not(feature = "profile"))]
    false
}

/// Whether `CLEAN_RUNLOG` is on (`profile` builds).
pub(crate) fn debug_runlog() -> bool {
    #[cfg(feature = "profile")]
    {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("CLEAN_RUNLOG").is_some())
    }
    #[cfg(not(feature = "profile"))]
    false
}

/// `CLEAN_RUNLOG` (`profile` builds): each run's node, operation, start
/// and end (µs since the first run logged) and what came of it, a line
/// each.
pub(crate) fn runlog(id: Id, op: &str, span: Option<(std::time::Instant, std::time::Instant)>, what: &str) {
    #[cfg(feature = "profile")]
    {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        if !*ON.get_or_init(|| std::env::var_os("CLEAN_RUNLOG").is_some()) {
            return;
        }
        let t0 = *T0.get_or_init(std::time::Instant::now);
        let us = |t: std::time::Instant| t.saturating_duration_since(t0).as_micros();
        let (a, b) = span.map_or((0, 0), |(a, b)| (us(a), us(b)));
        eprintln!("runlog {id} {op} {a} {b} {} {what}", us(std::time::Instant::now()));
    }
    #[cfg(not(feature = "profile"))]
    let _ = (id, op, span, what);
}

/// The wall clock of the Report's merge and link times (none on wasm32).
pub(crate) fn now() -> Option<std::time::Instant> {
    #[cfg(not(target_arch = "wasm32"))]
    return Some(std::time::Instant::now());
    #[cfg(target_arch = "wasm32")]
    None
}

/// Nanoseconds since `t` (0 without a clock).
pub(crate) fn ns_since(t: Option<std::time::Instant>) -> u64 {
    t.map_or(0, |t| u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX))
}

/// The interned texts of one hash: one, held in place (no allocation), or
/// those that collide.
pub enum Texts {
    One(Arc<Text>),
    Many(Vec<Arc<Text>>),
}

impl Default for Texts {
    fn default() -> Texts {
        Texts::Many(Vec::new())
    }
}

impl Texts {
    pub fn iter(&self) -> std::slice::Iter<'_, Arc<Text>> {
        match self {
            Texts::One(x) => std::slice::from_ref(x).iter(),
            Texts::Many(v) => v.iter(),
        }
    }

    pub fn push(&mut self, x: Arc<Text>) {
        match self {
            Texts::Many(v) if v.is_empty() => *self = Texts::One(x),
            Texts::Many(v) => v.push(x),
            Texts::One(y) => *self = Texts::Many(vec![y.clone(), x]),
        }
    }

    pub fn retain(&mut self, f: impl FnMut(&Arc<Text>) -> bool) {
        let mut v: Vec<Arc<Text>> = match std::mem::take(self) {
            Texts::One(x) => vec![x],
            Texts::Many(v) => v,
        };
        v.retain(f);
        *self = match v.len() {
            1 => Texts::One(v.pop().expect("one")),
            _ => Texts::Many(v),
        };
    }

    pub fn len(&self) -> usize {
        match self {
            Texts::One(_) => 1,
            Texts::Many(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Phase timers (`CLEAN_PROF`): nanoseconds per phase, summed.
pub(crate) mod prof {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    pub(crate) const NAMES: [&str; 48] = [
        "op", "apply", "settle_claims", "settle", "frames", "held_by", "prepare", "unguess", "absorb", "commit", "place", "reads", "defs", "contribs", "claimed", "outs",
        "set_contrib", "range_readers",
        "read_at", "pset_between", "flush", "reaching",
        // (Steps: choosing batches, the executor's calls, all of it.)
        "select", "execute", "steps",
        // (Filing reads, `reads` split: the keys made, entries looked up,
        // filed or unfiled by kind, scope and sibling windows.)
        "rd_keys", "rd_has", "rd_field", "rd_out", "rd_range", "rd_agg",
        // (A commit's node work: nodes made, their declared entries filed,
        // structure readers re-asked; entries filed and the readers they
        // re-ask; the committed node stored.)
        "create", "file_decl", "shape_at", "file_if", "def_readers", "put_node",
        // (A node stored, `World::put`: its pending-set entries, its holder
        // and claim entries, the in-work index, the contributors' index,
        // the node map.)
        "put_pset", "put_claims", "put_work", "put_acc", "put_map",
        // (A stop: all of it; its unfolding committed; what it is past;
        // its kept reads indexed; kept states' reads re-asked.)
        "stop", "unfold", "passed", "kept_ix", "kept_ask",
        // (Renumberings.)
        "relabel",
    ];
    pub(crate) static NS: [AtomicU64; 48] = [const { AtomicU64::new(0) }; 48];
    /// Whether `CLEAN_CONFLICTS` asks for the conflict counts (what keys
    /// a step's commits changed under its later results; `profile` builds
    /// only): every change is logged by key for them.
    #[inline]
    pub(crate) fn conflicts() -> bool {
        #[cfg(feature = "profile")]
        {
            static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            *ON.get_or_init(|| std::env::var_os("CLEAN_CONFLICTS").is_some())
        }
        #[cfg(not(feature = "profile"))]
        false
    }

    /// Whether `CLEAN_PROF` asks for them (`profile` builds only: without
    /// the feature every probe compiles away).
    #[inline]
    pub(crate) fn on() -> bool {
        #[cfg(feature = "profile")]
        {
            static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            *ON.get_or_init(|| std::env::var_os("CLEAN_PROF").is_some())
        }
        #[cfg(not(feature = "profile"))]
        false
    }
    /// Times `f` under phase `i`.
    #[inline]
    pub(crate) fn time<T>(i: usize, f: impl FnOnce() -> T) -> T {
        if !on() {
            return f();
        }
        let t = std::time::Instant::now();
        let r = f();
        NS[i].fetch_add(u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX), Relaxed);
        r
    }
    /// A phase start, for spans `time` cannot wrap.
    #[inline]
    pub(crate) fn start() -> Option<std::time::Instant> {
        on().then(std::time::Instant::now)
    }
    pub(crate) fn stop(i: usize, t: Option<std::time::Instant>) {
        if let Some(t) = t {
            NS[i].fetch_add(u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX), Relaxed);
        }
    }
    /// Times its scope under phase `i` (dropped at its end).
    pub(crate) struct Span(usize, Option<std::time::Instant>);
    impl Span {
        #[inline]
        pub(crate) fn new(i: usize) -> Self {
            Span(i, start())
        }
    }
    impl Drop for Span {
        #[inline]
        fn drop(&mut self) {
            stop(self.0, self.1);
        }
    }
    /// Adds `n` to event counter `i`.
    #[inline]
    pub(crate) fn add(i: usize, n: u64) {
        if on() {
            CN[i].fetch_add(n, Relaxed);
        }
    }
    /// Event counters (shown with the timers).
    pub(crate) const COUNTS: [&str; 48] = [
        "results", "clean", "data_conflict", "shape_conflict", "placing", "effect_change", "deferred", "st_kids", "st_effect", "st_claim", "st_env", "st_stop", "flushes",
        // (Rechecks queued, by source; reads settle asked.)
        "rk_freed", "rk_shape", "rk_out", "rk_kind", "rk_moved", "rk_outchg", "rk_field", "rk_reads", "rk_acc",
        // (The lane: runs past its first; why it stopped.)
        "lane_runs", "lane_none", "lane_end", "lane_held",
        // (Filing reads: reads committed, keys made, duplicate keys among
        // them, entries filed or unfiled, reruns whose old entries were
        // made again from their reads, not as filed.)
        "rd_reads", "rd_keyn", "rd_dups", "rd_filed", "rd_refile",
        // (Kept states asked in full, why: no index, another log branch, a
        // renumbering, fewer reads, changes forgotten, a change of all;
        // the reads asked so.)
        "kx_none", "kx_branch", "kx_relabel", "kx_short", "kx_forgot", "kx_all", "kx_reads",
        // (Placing: placements committed, made new, moved; extents walked:
        // follow nodes stepped over, continuations scanned.)
        "pl_specs", "pl_made", "pl_moved", "ext_follow", "ext_scan",
        "relabels",
        // (The lane took a job of its step no worker had; read hints asked,
        // found waiting, their fields asked.)
        "lane_took", "hb_calls", "hb_blocked", "hb_fields",
    ];
    pub(crate) static CN: [AtomicU64; 48] = [const { AtomicU64::new(0) }; 48];
    #[inline]
    pub(crate) fn count(i: usize) {
        if on() {
            CN[i].fetch_add(1, Relaxed);
        }
    }
    /// The phase timers now (a commit's start, for `by_op`).
    pub(crate) fn snapshot() -> Option<(std::time::Instant, [u64; 48], [u64; 44])> {
        on().then(|| (std::time::Instant::now(), std::array::from_fn(|i| NS[i].load(Relaxed)), std::array::from_fn(|i| CN[i].load(Relaxed))))
    }
    /// Commits by op: how many, their time, and the phases' share of it.
    #[allow(clippy::type_complexity)]
    static BY_OP: std::sync::Mutex<Option<std::collections::HashMap<String, (u64, u64, [u64; 48], [u64; 44])>>> = std::sync::Mutex::new(None);
    /// A commit of an `op` run, since `s0` (`snapshot`), done.
    pub(crate) fn by_op(op: &str, s0: Option<(std::time::Instant, [u64; 48], [u64; 44])>) {
        let Some((t, s0, c0)) = s0 else { return };
        let ns = u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let mut g = BY_OP.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let e = g.get_or_insert_with(Default::default).entry(op.to_string()).or_insert((0, 0, [0; 48], [0; 44]));
        e.0 += 1;
        e.1 += ns;
        for (i, x) in e.2.iter_mut().enumerate() {
            *x += NS[i].load(Relaxed).saturating_sub(s0[i]);
        }
        for (i, x) in e.3.iter_mut().enumerate() {
            *x += CN[i].load(Relaxed).saturating_sub(c0[i]);
        }
    }
    pub(crate) fn dump() {
        if !on() {
            return;
        }
        if let Some(m) = BY_OP.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
            let mut v: Vec<_> = m.into_iter().collect();
            v.sort_by_key(|e| std::cmp::Reverse(e.1.1));
            for (op, (n, ns, ph, cn)) in v.into_iter().take(12) {
                let mut p: Vec<(usize, u64)> = ph.iter().copied().enumerate().filter(|e| e.1 > 0).collect();
                p.sort_by_key(|e| std::cmp::Reverse(e.1));
                let p: Vec<String> = p.iter().take(14).map(|(i, x)| format!("{} {:.1}", NAMES[*i], *x as f64 / 1e6)).collect();
                let c: Vec<String> = cn.iter().enumerate().filter(|e| *e.1 > 0).map(|(i, x)| format!("{} {x}", COUNTS[i])).collect();
                eprintln!("clean commits by op: {op}: {n} commits {:.1}ms ({:.1}us each): {} | {}", ns as f64 / 1e6, ns as f64 / 1e3 / n.max(1) as f64, p.join(", "), c.join(", "));
            }
        }
        let c: Vec<String> = COUNTS.iter().zip(&CN).map(|(n, x)| format!("{n} {}", x.swap(0, Relaxed))).collect();
        eprintln!("clean counts: {}", c.join(", "));
        let v: Vec<String> = NAMES.iter().zip(&NS).map(|(n, x)| format!("{n} {:.1}ms", x.swap(0, Relaxed) as f64 / 1e6)).collect();
        eprintln!("clean prof: {}", v.join(", "));
    }
}
