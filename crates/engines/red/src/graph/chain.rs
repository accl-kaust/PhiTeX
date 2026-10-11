// The main chain and the node store (included into graph.rs: one module).
/// Reads the graph does not answer from env: `Key::Page`, `Key::File`,
/// `Key::JobName`, `Key::Given` (page fold, fixpoint driver, build).
pub trait Answers: Send + Sync {
    /// `None`: not answered here (the graph falls back: files from disk,
    /// the rest from env).
    fn answer(&self, g: &Graph, key: &Key, at: &Thunk) -> Option<Val>;
}

#[derive(Clone)]
pub struct GraphOpts {
    pub etex: bool,
    pub pdf: bool,
    pub names: Arc<Names>,
    pub answers: Option<Arc<dyn Answers>>,
    /// Memoise nodes on (input, reads).
    pub memo: bool,
}

impl Default for GraphOpts {
    fn default() -> Self {
        GraphOpts { etex: true, pdf: true, names: Arc::new(Names::default()), answers: None, memo: true }
    }
}

#[derive(Default)]
struct Arena {
    nodes: Vec<Arc<Node>>,
    prev: Vec<NodeId>,
}

#[derive(Default)]
struct Texts {
    list: Vec<(Arc<str>, Arc<str>)>,
    ids: HashMap<(Arc<str>, Arc<str>), TextId>,
}

pub struct Graph {
    pub etex: bool,
    pub pdf: bool,
    pub names: Arc<Names>,
    answers: RwLock<Option<Arc<dyn Answers>>>,
    arena: RwLock<Arena>,
    texts: Mutex<Texts>,
    pub memo: crate::memo::Memo,
    memo_on: bool,
    halted: AtomicBool,
    relax: Option<PrimId>,
    // ---- the build's main chain (api.rs / fixpoint.rs / edit.rs)
    root: RwLock<Option<Thunk>>,
    chain: RwLock<Vec<Arc<Node>>>,
    /// The texts as the chain was last reduced from them.
    reduced_texts: RwLock<HashMap<TextId, Arc<str>>>,
    finder: RwLock<Option<Finder>>,
    // ---- SSA bookkeeping
    /// def → the nodes that read it (`Node::uses`), for definitions made in
    /// this graph (ids ≥ `first_def`: format and initial values have none).
    users: RwLock<HashMap<crate::env::DefId, Vec<NodeId>>>,
    first_def: crate::env::DefId,
    /// Edit epochs: `maps[e]` maps positions of epoch `e` to epoch `e + 1`.
    maps: RwLock<Vec<Arc<crate::edit::EditMap>>>,
}

/// The file finder: a name as TeX asks for it → (the found name, its bytes).
pub type Finder = Arc<dyn Fn(&str) -> Option<(Arc<str>, Arc<[u8]>)> + Send + Sync>;

impl Graph {
    pub fn new(opts: GraphOpts) -> Graph {
        Graph {
            etex: opts.etex,
            pdf: opts.pdf,
            names: opts.names,
            answers: RwLock::new(opts.answers),
            arena: RwLock::new(Arena::default()),
            texts: Mutex::new(Texts::default()),
            memo: crate::memo::Memo::default(),
            memo_on: opts.memo,
            halted: AtomicBool::new(false),
            relax: crate::prim::registry().by_name("relax"),
            root: RwLock::new(None),
            chain: RwLock::new(Vec::new()),
            reduced_texts: RwLock::new(HashMap::new()),
            finder: RwLock::new(None),
            users: RwLock::new(HashMap::new()),
            first_def: crate::env::next_def_id(),
            maps: RwLock::new(Vec::new()),
        }
    }

    // ---- SSA: def-use edges, epochs

    /// Records node `id`'s use edges.
    pub fn register_uses(&self, id: NodeId, uses: &[(Key, crate::env::DefId)]) {
        let mut u = self.users.write().unwrap();
        for (_, d) in uses {
            if *d >= self.first_def {
                u.entry(*d).or_default().push(id);
            }
        }
    }
    /// Definition `n` (made by an edit) now stands for the equal `o` (the
    /// chains met): `n`'s readers become `o`'s.
    pub fn forward_def(&self, n: &Arc<crate::env::DefCell>, o: &Arc<crate::env::DefCell>) {
        let (ni, oi) = (n.id_now(), o.id_now());
        if ni == oi {
            return;
        }
        n.forward(o);
        let mut u = self.users.write().unwrap();
        if let Some(v) = u.remove(&ni) {
            u.entry(oi).or_default().extend(v);
        }
    }
    /// The nodes that read definition `d` (some may no longer be in the chain).
    pub fn users_of(&self, d: crate::env::DefId) -> Vec<NodeId> {
        self.users.read().unwrap().get(&d).cloned().unwrap_or_default()
    }
    /// The current edit epoch.
    pub fn epoch(&self) -> u32 {
        self.maps.read().unwrap().len() as u32
    }
    /// Starts a new epoch: positions of the current one map through `m`.
    pub fn push_epoch(&self, m: crate::edit::EditMap) {
        self.maps.write().unwrap().push(Arc::new(m));
    }
    /// The maps from epoch `e` to now.
    pub fn maps_since(&self, e: u32) -> Vec<Arc<crate::edit::EditMap>> {
        self.maps.read().unwrap()[(e as usize).min(self.epoch() as usize)..].to_vec()
    }
    /// A span of epoch `e`, in the current epoch's positions.
    pub fn span_now(&self, s: Span, e: u32) -> Span {
        self.maps_since(e).iter().fold(s, |s, m| m.span(s))
    }
    /// An input of epoch `e`, in the current epoch's positions and texts.
    pub fn input_now(&self, i: &Input, e: u32) -> Input {
        let ms = self.maps_since(e);
        if ms.is_empty() {
            return i.clone();
        }
        ms.iter().fold(i.clone(), |i, m| m.input(&i))
    }
    /// The thunk a node of the chain started from, in the current epoch.
    pub fn thunk_before(&self, chain: &[Arc<Node>], start: &Thunk, i: usize) -> Thunk {
        if i == 0 {
            Thunk { env: start.env.clone(), input: self.input_now(&start.input, 0) }
        } else {
            let p = &chain[i - 1];
            Thunk { env: p.next.env.clone(), input: self.input_now(&p.next.input, p.epoch) }
        }
    }

    /// Re-reduces at the place of `old` from `t`: when the new node makes the
    /// same shape of definitions, it keeps `old`'s definition cells (values
    /// updated in place) and returns the definitions whose value changed;
    /// `None`: the shape differs.
    pub fn reduce_reusing(&self, t: &Thunk, old: &Node) -> (Arc<Node>, Option<Vec<crate::env::DefId>>) {
        let mut r = Reducing::new(self, t);
        let op = r.step();
        r.finish_reusing(op, Some(old))
    }

    // ---- the build's main chain

    /// Files are looked up through `f` (else read from disk).
    pub fn set_finder(&self, f: Finder) {
        *self.finder.write().unwrap() = Some(f);
    }
    /// An input file's bytes through the finder (or the disk).
    pub fn find(&self, name: &str) -> Option<Arc<[u8]>> {
        let f = self.finder.read().unwrap().clone();
        match f {
            Some(f) => f(name).map(|(_, b)| b),
            None => std::fs::read(name).ok().map(Arc::from),
        }
    }
    /// Registers a source text; its id.
    pub fn add_text(&self, name: Arc<str>, data: Arc<str>) -> TextId {
        self.intern_text(name, data)
    }
    /// Text `id` is now `data` (the chain still holds the old one until re-reduced).
    pub fn set_text(&self, id: TextId, data: Arc<str>) {
        let mut t = self.texts.lock().unwrap();
        if let Some(e) = t.list.get_mut(id as usize) {
            e.1 = data;
        }
    }
    /// Text `id` as the chain was last reduced from it.
    pub fn reduced_text(&self, id: TextId) -> Option<Arc<str>> {
        self.reduced_texts.read().unwrap().get(&id).cloned()
    }
    /// The root thunk of the main chain; the chain is emptied.
    pub fn set_root(&self, t: Thunk) {
        *self.root.write().unwrap() = Some(t);
        self.chain.write().unwrap().clear();
    }
    /// The main chain's nodes, in order.
    pub fn nodes(&self) -> Vec<Arc<Node>> {
        self.chain.read().unwrap().clone()
    }
    /// The main chain as `edit.rs` takes it.
    pub fn chain(&self) -> crate::edit::Chain {
        let start = self.root.read().unwrap().clone().unwrap_or_else(|| self.start(self.initial_env(), Input::default()));
        crate::edit::Chain { start, nodes: self.chain.read().unwrap().clone() }
    }
    /// Replaces the main chain (after `edit::update`).
    pub fn set_chain(&self, c: crate::edit::Chain) {
        *self.root.write().unwrap() = Some(c.start);
        *self.chain.write().unwrap() = c.nodes;
    }
    /// Keeps the first `n` nodes of the main chain.
    pub fn truncate_chain(&self, n: usize) {
        self.chain.write().unwrap().truncate(n);
    }
    /// Nodes made in this graph so far (memo hits included).
    pub fn nodes_reduced(&self) -> usize {
        self.len()
    }
    /// The thunk after the main chain's last node.
    fn chain_end(&self) -> Option<Thunk> {
        match self.chain.read().unwrap().last() {
            Some(n) => Some(n.next.clone()),
            None => self.root.read().unwrap().clone(),
        }
    }
    /// The env after the main chain (the job's end env once reduced).
    pub fn end_env(&self) -> Env {
        self.chain_end().map_or_else(|| self.initial_env(), |t| t.env)
    }
    /// Reduces the main chain to the job's end (sequentially).
    pub fn reduce(&self) {
        {
            let t = self.texts.lock().unwrap();
            let mut r = self.reduced_texts.write().unwrap();
            for (i, (_, d)) in t.list.iter().enumerate() {
                r.insert(i as TextId, d.clone());
            }
        }
        let Some(mut t) = self.chain_end() else { return };
        while !Graph::done(&t) && !self.halted() {
            let n = self.reduce_one(&t);
            t = n.next.clone();
            self.chain.write().unwrap().push(n);
        }
    }

    pub fn set_answers(&self, a: Arc<dyn Answers>) {
        *self.answers.write().unwrap() = Some(a);
    }

    /// The INITEX env of this session's engine.
    pub fn initial_env(&self) -> Env {
        initial_env(&self.names, self.etex, self.pdf)
    }

    pub fn start(&self, env: Env, input: Input) -> Thunk {
        Thunk { env, input }
    }

    pub fn halt(&self) {
        self.halted.store(true, Ordering::Relaxed);
    }
    pub fn resume(&self) {
        self.halted.store(false, Ordering::Relaxed);
    }
    pub fn halted(&self) -> bool {
        self.halted.load(Ordering::Relaxed)
    }

    // ---- texts
    pub fn intern_text(&self, name: Arc<str>, data: Arc<str>) -> TextId {
        let mut t = self.texts.lock().unwrap();
        if let Some(&id) = t.ids.get(&(name.clone(), data.clone())) {
            return id;
        }
        let id = t.list.len() as TextId;
        t.list.push((name.clone(), data.clone()));
        t.ids.insert((name, data), id);
        id
    }
    pub fn text(&self, id: TextId) -> Option<(Arc<str>, Arc<str>)> {
        self.texts.lock().unwrap().list.get(id as usize).cloned()
    }

    // ---- the arena
    pub fn node(&self, id: NodeId) -> Option<Arc<Node>> {
        self.arena.read().unwrap().nodes.get(id as usize).cloned()
    }
    /// The node before `id` in its chain.
    pub fn prev(&self, id: NodeId) -> NodeId {
        self.arena.read().unwrap().prev.get(id as usize).copied().unwrap_or(NO_NODE)
    }
    /// A node's id (its next env names it).
    pub fn id_of(n: &Node) -> NodeId {
        n.next.env.inner.at
    }
    pub fn len(&self) -> usize {
        self.arena.read().unwrap().nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Appends a node after thunk `at` (used for memo hits, and by the page
    /// builder / output for `Op::Fire`). Its next env is `at.env + delta`.
    /// Its definitions get fresh cells; its use edges are the definitions
    /// reaching its reads at `at`.
    pub fn push_node(&self, at: &Thunk, op: Op, spans: Vec<Span>, reads: Vec<(Key, Val)>, out: Out, input: Input) -> Arc<Node> {
        let (env, cells) = env_apply_collect(&at.env, &out.delta);
        let uses: Vec<(Key, crate::env::DefId)> =
            reads.iter().map(|(k, _)| (k.clone(), at.env.def_of(k))).filter(|(_, d)| *d != 0).collect();
        let mut a = self.arena.write().unwrap();
        let id = a.nodes.len() as NodeId;
        let n = Arc::new(Node {
            op,
            spans,
            reads,
            uses,
            cells,
            out,
            next: Thunk { env: env_at(&env, id), input },
            epoch: self.epoch(),
        });
        a.nodes.push(n.clone());
        a.prev.push(at.env.inner.at);
        drop(a);
        self.register_uses(id, &n.uses);
        n
    }

    /// A node reduced elsewhere (a speculative chain) placed after `at`: its
    /// definitions get fresh cells, its use edges the definitions reaching
    /// its reads at `at`, its next input `input`.
    pub fn rebase(&self, at: &Thunk, n: &Node, input: Input) -> Arc<Node> {
        self.push_node(at, n.op.clone(), n.spans.clone(), n.reads.clone(), n.out.clone(), input)
    }

    /// Reduces `t` into exactly one node.
    pub fn reduce_one(&self, t: &Thunk) -> Arc<Node> {
        if self.memo_on {
            if let Some(old) = self.memo.find(self, t) {
                return self.push_node(t, old.op.clone(), old.spans.clone(), old.reads.clone(), old.out.clone(), old.next.input.clone());
            }
        }
        let mut r = Reducing::new(self, t);
        let op = r.step();
        let n = r.finish(op);
        if self.memo_on && !self.halted() {
            self.memo.insert(&t.input, n.clone());
        }
        n
    }

    /// Reduces `t` into one node with answers `ans` over the graph's and a
    /// cancel flag (speculation); never memoised.
    pub fn reduce_with<'a>(&'a self, t: &Thunk, ans: &'a dyn Answers, cancel: &'a AtomicBool) -> Arc<Node> {
        let mut r = Reducing::new(self, t);
        r.over = Some(ans);
        r.cancel = Some(cancel);
        let op = r.step();
        r.finish(op)
    }

    /// The job has nothing more to reduce from `t`.
    pub fn done(t: &Thunk) -> bool {
        t.input.is_empty() || t.env.lookup(&Key::Ended).int() != 0
    }

    /// Reduces from `t` until the input is exhausted, the job ended, `stop`
    /// says so (the stopping node included), or the graph is halted.
    pub fn run(&self, t: Thunk, stop: &dyn Fn(&Node) -> bool) -> Vec<Arc<Node>> {
        let mut out = Vec::new();
        let mut t = t;
        while !Graph::done(&t) && !self.halted() {
            let n = self.reduce_one(&t);
            out.push(n.clone());
            if stop(&n) {
                break;
            }
            t = n.next.clone();
        }
        out
    }

    /// The items of the innermost list open at `t`.
    pub fn list_items(&self, t: &Thunk) -> Vec<Item> {
        self.ext_suffix(t.env.inner.at, 0, usize::MAX).items.to_vec()
    }
    /// The items of the list `c` levels out from the innermost at `t`.
    pub fn list_items_up(&self, t: &Thunk, c: u32) -> Vec<Item> {
        self.ext_suffix(t.env.inner.at, c, usize::MAX).items.to_vec()
    }

    /// The last `k` items (all when fewer) of the list `c` levels out at the
    /// end of node `at`, gathered back along the chain.
    pub fn ext_suffix(&self, at: NodeId, c: u32, k: usize) -> ItemsDatum {
        let mut d = 0u32;
        let mut u = 0u32;
        let mut pending: Vec<u8> = Vec::new();
        let mut out: Vec<Item> = Vec::new();
        let mut whole = true;
        let mut id = at;
        'chain: while id != NO_NODE {
            let Some(n) = self.node(id) else { break };
            for e in n.out.emits.iter().rev() {
                match e {
                    Emit::Close => d += 1,
                    Emit::Open(_) => {
                        if d > 0 {
                            d -= 1;
                        } else {
                            if u == c {
                                break 'chain;
                            }
                            u += 1;
                        }
                    }
                    Emit::Item(it) if d == 0 && u == c => {
                        let mut removed = false;
                        while let Some(kind) = pending.pop() {
                            if kind_matches(it, kind) {
                                removed = true;
                                break;
                            }
                        }
                        if !removed {
                            out.push(it.clone());
                            if out.len() >= k {
                                whole = false;
                                break 'chain;
                            }
                        }
                    }
                    Emit::RemoveTail(kind) if d == 0 && u == c => pending.push(*kind),
                    _ => {}
                }
            }
            id = self.prev(id);
        }
        out.reverse();
        ItemsDatum { items: out.into(), whole }
    }

    fn ext_holds(&self, t: &Thunk, c: u32, v: &Val) -> bool {
        let Val::Data(d) = v else { return false };
        let Some(rec) = d.as_any().downcast_ref::<ItemsDatum>() else { return false };
        let k = if rec.whole { usize::MAX } else { rec.items.len().max(1) };
        let cand = self.ext_suffix(t.env.inner.at, c, k);
        cand.items == rec.items && (!rec.whole || cand.whole)
    }

    /// The answer to an answered key at `t`.
    pub fn answer(&self, key: &Key, t: &Thunk) -> Val {
        let a = self.answers.read().unwrap().clone();
        if let Some(a) = a {
            if let Some(v) = a.answer(self, key, t) {
                return v;
            }
        }
        match key {
            Key::File(n) => match self.find(n) {
                Some(b) => Val::Bytes(b),
                None => Val::Void,
            },
            Key::LineNo => Val::Int(line_of(&t.input)),
            // GAP: with no page fold answering (no `sched::Sched` driving the
            // build), the page reads as empty.
            Key::Other(tg, crate::page::EMPTY) if *tg == crate::prim::tag::PAGE => Val::Int(1),
            _ => t.env.lookup(key),
        }
    }

    /// Does the read `(k, v)` still hold at thunk `t`?
    pub fn holds(&self, t: &Thunk, k: &Key, v: &Val) -> bool {
        match k {
            Key::Tail => self.ext_holds(t, 0, v),
            Key::Other(pk::TAG, n) if pk::class(*n) == pk::class(pk::CHAIN) => self.ext_holds(t, n & 0xffff, v),
            Key::LineNo => Val::Int(line_of(&t.input)) == *v,
            Key::Page(_) | Key::File(_) | Key::JobName | Key::Given(_) => self.answer(k, t) == *v,
            Key::Other(tg, _) if *tg == crate::prim::tag::PAGE => self.answer(k, t) == *v,
            _ => t.env.lookup(k) == *v,
        }
    }

    /// Do all of a node's reads hold at `t`?
    pub fn reads_hold(&self, t: &Thunk, reads: &[(Key, Val)]) -> bool {
        reads.iter().all(|(k, v)| self.holds(t, k, v))
    }
}

/// A node's consumed spans.
pub fn node_spans(n: &Node) -> &[Span] {
    &n.spans
}

/// TeX's `line` of the innermost text in `input` (0 before its first line).
pub fn line_of(input: &Input) -> i32 {
    let mut i = input.clone();
    loop {
        match i.top() {
            None => return 0,
            Some(Src::Text { data, line_end, pos, .. }) => {
                if *line_end == 0 && *pos == 0 {
                    return 0;
                }
                let b = data.as_bytes();
                let mut p = (*line_end as usize).min(b.len());
                if p > 0 && b[p - 1] == b'\n' {
                    p -= 1;
                }
                return b[..p].iter().filter(|&&c| c == b'\n').count() as i32 + 1;
            }
            Some(_) => i = i.below(),
        }
    }
}

fn merge_span(v: &mut Vec<Span>, s: Span) {
    if let Some(l) = v.last_mut() {
        if l.text == s.text && l.to == s.from {
            l.to = s.to;
            return;
        }
    }
    v.push(s);
}

// ============================================================== reducing

