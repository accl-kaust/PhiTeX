//! What every world shares: the value table, the operations and their
//! static facts, projections, accumulators, loop-back declarations and the
//! environment.

use std::sync::{Arc, Mutex, RwLock};

use super::world::{Id, May, Static, Text, World};
use crate::api::{EvalError, Monoid, OpId, Operation, Projection};
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

impl Env {
    /// What a read of `f` finds: `Some(answer)`, or `None` unknown.
    #[allow(clippy::option_option)]
    pub(crate) fn get(&self, f: Field) -> Option<Option<ValueId>> {
        if let Some(&x) = self.known.get(&f) {
            return Some(Some(x));
        }
        if self.absent.contains(&f) {
            return Some(None);
        }
        self.provided.get(&f).copied()
    }
}

pub(crate) struct Core {
    pub values: Values,
    pub ops: Vec<Arc<dyn Operation>>,
    pub op_names: Vec<&'static str>,
    pub projs: Vec<Projection>,
    pub accs: FxHashMap<Field, Arc<dyn Monoid>>,
    pub seqs: FxHashSet<Field>,
    pub stable: FxHashSet<Field>,
    /// Declared loop-back fields and their initial values.
    pub loops: FxHashMap<Field, ValueId>,
    /// Loop-backs declared by runs, taken in when the evaluation is quiet.
    pub loops_later: Mutex<Vec<(Field, ValueId)>>,
    pub env: RwLock<Env>,
    pub provider: Option<Box<crate::api::EnvProvider>>,
    /// Texts analysed, by `(op, args)` hash.
    pub texts: Mutex<FxHashMap<u64, Vec<Arc<Text>>>>,
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
}

impl Core {
    pub(crate) fn new() -> Core {
        Core {
            values: Values::new(),
            ops: Vec::new(),
            op_names: Vec::new(),
            projs: Vec::new(),
            accs: FxHashMap::default(),
            seqs: FxHashSet::default(),
            stable: FxHashSet::default(),
            loops: FxHashMap::default(),
            loops_later: Mutex::new(Vec::new()),
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
        }
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
        let kept = t.values().map(Vec::len).sum();
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
                    c.1 = self.accs.get(&f).map_or(x, |m| m.combine(v, c.1, x));
                } else {
                    out.push((f, x));
                }
            }
            Arc::from(out)
        });
        let preds = self
            .hook(who, op, "predicted_defs", || None, || o.predicted_defs(args, v))
            .map(Arc::from);
        let fin = self.hook(who, op, "final_only", || false, || o.final_only(args, v));
        let contribs_complete = contribs.is_some() && self.hook(who, op, "contribs_complete", || false, || o.contribs_complete(args, v));
        Static {
            defs: defs.map(Arc::from),
            may,
            effect,
            contribs,
            contribs_complete,
            preds,
            fin,
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
    pub(crate) fn env_get(&self, _w: &World, f: Field) -> Option<Option<ValueId>> {
        {
            let e = self.env.read().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(a) = e.get(f) {
                return Some(a);
            }
            if e.unknown.contains(&f) {
                return None;
            }
        }
        let p = self.provider.as_ref()?;
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
        self.env.read().unwrap_or_else(std::sync::PoisonError::into_inner).get(f)
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

    pub(crate) fn declare_loop_later(&self, f: Field, init: ValueId) {
        if !self.loops.contains_key(&f) {
            lock(&self.loops_later).push((f, init));
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

/// Whether `CLEAN_DEBUG` asks for traces (read once).
pub(crate) fn debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CLEAN_DEBUG").is_some())
}

/// Phase timers (`CLEAN_PROF`): nanoseconds per phase, summed.
pub(crate) mod prof {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    pub(crate) const NAMES: [&str; 25] = [
        "op", "apply", "settle_claims", "settle", "frames", "held_by", "prepare", "unguess", "absorb", "commit", "place", "reads", "defs", "contribs", "claimed", "outs",
        "set_contrib", "range_readers",
        "read_at", "pset_between", "flush", "reaching",
        // (Steps: choosing batches, the executor's calls, all of it.)
        "select", "execute", "steps",
    ];
    pub(crate) static NS: [AtomicU64; 25] = [const { AtomicU64::new(0) }; 25];
    pub(crate) fn on() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("CLEAN_PROF").is_some())
    }
    /// Times `f` under phase `i`.
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
    pub(crate) fn start() -> Option<std::time::Instant> {
        on().then(std::time::Instant::now)
    }
    pub(crate) fn stop(i: usize, t: Option<std::time::Instant>) {
        if let Some(t) = t {
            NS[i].fetch_add(u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX), Relaxed);
        }
    }
    /// Event counters (shown with the timers).
    pub(crate) const COUNTS: [&str; 13] = [
        "results", "clean", "data_conflict", "shape_conflict", "placing", "effect_change", "deferred", "st_kids", "st_effect", "st_claim", "st_env", "st_stop", "flushes",
    ];
    pub(crate) static CN: [AtomicU64; 13] = [const { AtomicU64::new(0) }; 13];
    pub(crate) fn count(i: usize) {
        if on() {
            CN[i].fetch_add(1, Relaxed);
        }
    }
    pub(crate) fn dump() {
        if !on() {
            return;
        }
        let c: Vec<String> = COUNTS.iter().zip(&CN).map(|(n, x)| format!("{n} {}", x.swap(0, Relaxed))).collect();
        eprintln!("clean counts: {}", c.join(", "));
        let v: Vec<String> = NAMES.iter().zip(&NS).map(|(n, x)| format!("{n} {:.1}ms", x.swap(0, Relaxed) as f64 / 1e6)).collect();
        eprintln!("clean prof: {}", v.join(", "));
    }
}
