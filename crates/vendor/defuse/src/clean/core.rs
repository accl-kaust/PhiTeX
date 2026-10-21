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
    /// Accumulators declared by field-name prefix, first declared first,
    /// and each field's resolution among them (made on first use).
    pub acc_prefixes: Vec<(String, Arc<dyn Monoid>)>,
    pub acc_memo: crate::sync::Table<Option<Arc<dyn Monoid>>>,
    pub seqs: FxHashSet<Field>,
    pub stable: FxHashSet<Field>,

    /// Declared loop-back fields and their initial values.
    pub loops: FxHashMap<Field, ValueId>,
    /// Loop-backs declared by runs, taken in when the evaluation is quiet.
    pub loops_later: Mutex<Vec<(Field, ValueId)>>,
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
        Core {
            values: Values::new(),
            ops: Vec::new(),
            op_names: Vec::new(),
            projs: Vec::new(),
            accs: FxHashMap::default(),
            acc_prefixes: Vec::new(),
            acc_memo: crate::sync::Table::new(),
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
            hold_fin: std::sync::atomic::AtomicBool::new(true),
            asking: Mutex::new(()),
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
        let _asking = lock(&self.asking);
        {
            // (Answered while waiting for the turn.)
            let e = self.env.read().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(a) = e.get(f) {
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
    pub(crate) const COUNTS: [&str; 44] = [
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
    ];
    pub(crate) static CN: [AtomicU64; 44] = [const { AtomicU64::new(0) }; 44];
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
