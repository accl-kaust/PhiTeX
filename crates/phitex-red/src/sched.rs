//! A2: demand from the PDF root, the parallel frontier, speculative reduction
//! of thunks on a predicted env validated on read sets.
//!
//! The PDF demands the main chain: `pending(env; input)` is reduced node by
//! node until the job ends (`Key::Ended` defined, or the input exhausted);
//! nothing after `\end` is ever demanded. Pure applications (line breaking,
//! packing, mlists, page bodies, PDF streams) are submitted by their callers
//! to the [`Pool`] as ready tasks and joined where their value is demanded.
//!
//! Speculation: likely boundaries in the main text (a paragraph start after a
//! blank line at brace depth 0, a line with `\input`, a `\begin{`) are reduced
//! ahead on workers, on a predicted env (the last build's env at that place,
//! else the env of the newest real node), with `\pagegoal`… answered by a
//! snapshot of the page fold ("no fire"). A speculative node is adopted only
//! when the real chain stands at the same text position with the same scope
//! structure and every read the node made equals the real value; the chain is
//! discarded from the first node that fails.
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use crate::env::{Change, Env};
use crate::graph::{Answers, Graph};
use crate::page::Fold;
use crate::term::{Emit, Input, Line, Node, Src, TextId, Thunk};
use crate::val::{Key, Val};

// ------------------------------------------------------------------ the pool

type Job = Box<dyn FnOnce() + Send + 'static>;

struct PoolInner {
    queue: Mutex<(VecDeque<Job>, bool)>,
    ready: Condvar,
}

impl PoolInner {
    fn take(&self) -> Option<Job> {
        self.queue.lock().unwrap().0.pop_front()
    }
}

/// A fixed set of worker threads running ready tasks in submission order.
pub struct Pool {
    inner: Arc<PoolInner>,
    workers: Vec<JoinHandle<()>>,
}

/// A submitted task's result, joined where it is demanded.
pub struct Task<T> {
    slot: Arc<(Mutex<Option<std::thread::Result<T>>>, Condvar)>,
    pool: Arc<PoolInner>,
}

impl Pool {
    pub fn new(n: usize) -> Pool {
        let inner = Arc::new(PoolInner { queue: Mutex::new((VecDeque::new(), false)), ready: Condvar::new() });
        let workers = (0..n.max(1))
            .map(|_| {
                let p = inner.clone();
                std::thread::spawn(move || loop {
                    let job = {
                        let mut q = p.queue.lock().unwrap();
                        loop {
                            if let Some(j) = q.0.pop_front() {
                                break Some(j);
                            }
                            if q.1 {
                                break None;
                            }
                            q = p.ready.wait(q).unwrap();
                        }
                    };
                    match job {
                        Some(j) => j(),
                        None => return,
                    }
                })
            })
            .collect();
        Pool { inner, workers }
    }

    pub fn threads(&self) -> usize {
        self.workers.len()
    }

    /// Submits a pure application; its value is had by `Task::join`.
    pub fn spawn<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> Task<T> {
        let slot = Arc::new((Mutex::new(None), Condvar::new()));
        let s = slot.clone();
        let job: Job = Box::new(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
            *s.0.lock().unwrap() = Some(r);
            s.1.notify_all();
        });
        self.inner.queue.lock().unwrap().0.push_back(job);
        self.inner.ready.notify_one();
        Task { slot, pool: self.inner.clone() }
    }

    /// Maps `f` over `xs` in parallel, results in order.
    pub fn map<A: Send + 'static, T: Send + 'static>(&self, xs: Vec<A>, f: Arc<dyn Fn(A) -> T + Send + Sync>) -> Vec<T> {
        let tasks: Vec<Task<T>> = xs.into_iter().map(|x| {
            let f = f.clone();
            self.spawn(move || f(x))
        }).collect();
        tasks.into_iter().map(Task::join).collect()
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.inner.queue.lock().unwrap().1 = true;
        self.inner.ready.notify_all();
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

impl<T> Task<T> {
    pub fn is_done(&self) -> bool {
        self.slot.0.lock().unwrap().is_some()
    }
    /// Waits for the value, running queued tasks meanwhile (a task joining
    /// another never starves the pool).
    pub fn join(self) -> T {
        loop {
            if let Some(r) = self.slot.0.lock().unwrap().take() {
                return r.unwrap_or_else(|e| std::panic::resume_unwind(e));
            }
            if let Some(j) = self.pool.take() {
                j();
                continue;
            }
            let g = self.slot.0.lock().unwrap();
            if g.is_none() {
                let _ = self.slot.1.wait_timeout(g, std::time::Duration::from_millis(2)).unwrap();
            }
        }
    }
}

// ------------------------------------------------------------- the answers

/// Answers the reads the env cannot: the page quantities, from the fold of
/// the main vertical list so far (a snapshot for speculation: "no fire").
struct PageAnswers {
    fold: Fold,
}

impl Answers for PageAnswers {
    fn answer(&self, _g: &Graph, key: &Key, _at: &Thunk) -> Option<Val> {
        self.fold.answer(key)
    }
}

// ------------------------------------------------------------- speculation

/// A likely boundary: a line start in a text.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Boundary {
    pub text: TextId,
    pub pos: u32,
}

/// Likely boundaries in a source text, in order: the first non-blank line
/// after a blank line at brace depth 0, a line containing `\input`, a line
/// starting with `\begin{` at brace depth 0. Comments and `\{` `\}` skipped.
pub fn boundaries(text: TextId, data: &str) -> Vec<Boundary> {
    let b = data.as_bytes();
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    let mut line_start = 0usize;
    let mut blank_before = false;
    while line_start < b.len() {
        let end = b[line_start..].iter().position(|&c| c == b'\n').map_or(b.len(), |e| line_start + e);
        let line = &b[line_start..end];
        let blank = line.iter().all(|c| c.is_ascii_whitespace());
        if !blank {
            let content = &line[line.iter().take_while(|c| **c == b' ' || **c == b'\t').count()..];
            let at_zero = depth == 0;
            let is_begin = content.starts_with(b"\\begin{");
            let has_input = find(line, b"\\input").is_some_and(|i| {
                !line[..i].contains(&b'%') && !line.get(i + 6).is_some_and(|c| c.is_ascii_alphabetic())
            });
            if line_start > 0 && ((blank_before && at_zero) || (is_begin && at_zero) || has_input) {
                out.push(Boundary { text, pos: line_start as u32 });
            }
            let mut i = 0;
            while i < line.len() {
                match line[i] {
                    b'\\' => i += 1,
                    b'%' => break,
                    b'{' => depth += 1,
                    b'}' => depth = (depth - 1).max(0),
                    _ => {}
                }
                i += 1;
            }
        }
        blank_before = blank;
        line_start = end + 1;
    }
    out
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

/// A speculative chain from a boundary: nodes reduced on a predicted env,
/// their inputs rooted on the boundary's text alone (rebased on adoption).
struct Chain {
    at: Boundary,
    /// The predicted env's scope structure, compared with the real one's.
    shape: Shape,
    nodes: Mutex<Vec<Node>>,
    done: AtomicBool,
    cancel: Arc<AtomicBool>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Shape {
    group_depth: u32,
    list_depth: u32,
    group_kind: crate::env::GroupKind,
    mode: crate::env::Mode,
}

fn shape(e: &Env) -> Shape {
    Shape { group_depth: e.group_depth(), list_depth: e.list_depth(), group_kind: e.group_kind(), mode: e.mode() }
}

/// The text source at the input head, if it is a text at a line start.
fn head_line_start(i: &Input) -> Option<(TextId, u32)> {
    match i.top()? {
        Src::Text { text, pos, line: Line::NewLine, ending: false, .. } => Some((*text, *pos)),
        _ => None,
    }
}

/// `i` with its bottom (the empty input) replaced by `base`.
fn rebase(i: &Input, base: &Input) -> Input {
    match &i.0 {
        None => base.clone(),
        Some(p) => rebase(&p.1, base).push(p.0.clone()),
    }
}

// ------------------------------------------------------------- the scheduler

/// Bounds and switches.
#[derive(Clone, Debug)]
pub struct Options {
    pub threads: usize,
    pub speculate: bool,
    /// Speculative chains in flight at once.
    pub max_chains: usize,
    /// Speculative nodes held at once (all chains): bounded memory.
    pub max_spec_nodes: usize,
    /// Nodes one chain reduces at most.
    pub chain_len: usize,
}

impl Default for Options {
    fn default() -> Options {
        let n = std::thread::available_parallelism().map_or(1, |n| n.get());
        Options { threads: n, speculate: n > 1, max_chains: n.saturating_sub(1).max(1), max_spec_nodes: 1 << 16, chain_len: 4096 }
    }
}

/// What a run gives: the main chain, how much speculation paid.
#[derive(Default, Debug)]
pub struct Stats {
    pub nodes: usize,
    pub adopted: usize,
    pub discarded: usize,
    pub chains: usize,
}

pub struct Sched {
    pub opts: Options,
    pub pool: Arc<Pool>,
    /// The last build's env at each line start the main chain stood at.
    last_envs: HashMap<Boundary, Env>,
    halt: Arc<AtomicBool>,
    spec_nodes: Arc<AtomicUsize>,
    pub stats: Stats,
}

impl Sched {
    pub fn new(opts: Options) -> Sched {
        let pool = Arc::new(Pool::new(opts.threads));
        Sched { opts, pool, last_envs: HashMap::new(), halt: Arc::new(AtomicBool::new(false)), spec_nodes: Arc::new(AtomicUsize::new(0)), stats: Stats::default() }
    }

    /// Stops the run (and every speculative chain) soon.
    pub fn halt(&self) {
        self.halt.store(true, Ordering::Relaxed);
    }
    pub fn halt_flag(&self) -> Arc<AtomicBool> {
        self.halt.clone()
    }

    /// Demands the main chain from `root` until the job ends. Returns its
    /// nodes in order and the page fold over the main vertical list.
    pub fn run(&mut self, graph: Arc<Graph>, root: Thunk) -> (Vec<Node>, Fold) {
        self.halt.store(false, Ordering::Relaxed);
        self.stats = Stats::default();
        let main_depth = root.env.list_depth();
        let mut fold = Fold::default();
        let mut chains: Vec<Arc<Chain>> = Vec::new();
        let mut pending: VecDeque<Boundary> = match root.input.top() {
            Some(Src::Text { text, data, .. }) if self.opts.speculate => boundaries(*text, data).into(),
            _ => VecDeque::new(),
        };
        let mut new_envs = HashMap::new();
        let mut out = Vec::new();
        let mut cur = root;

        loop {
            if self.halt.load(Ordering::Relaxed) || cur.input.is_empty() {
                break;
            }
            let here = head_line_start(&cur.input).map(|(text, pos)| Boundary { text, pos });
            if let Some(b) = here {
                new_envs.insert(b, cur.env.clone());
                // Chains behind us can never be adopted.
                chains.retain(|c| {
                    let keep = c.at.text != b.text || c.at.pos >= b.pos;
                    if !keep {
                        self.retire(c);
                    }
                    keep
                });
                while pending.front().is_some_and(|p| p.text == b.text && p.pos <= b.pos) {
                    pending.pop_front();
                }
            }
            // Keep the speculation window full.
            while self.opts.speculate && chains.len() < self.opts.max_chains {
                let Some(b) = pending.pop_front() else { break };
                if let Some(c) = self.start_chain(&graph, &cur, b, &fold) {
                    chains.push(c);
                }
            }
            // Adopt from a chain standing where we stand.
            if let Some(b) = here {
                if let Some(ix) = chains.iter().position(|c| c.at == b) {
                    let c = chains.remove(ix);
                    if c.shape == shape(&cur.env) {
                        let (n, next) = self.adopt(&c, &cur, main_depth, &mut fold, &mut out);
                        if n > 0 {
                            cur = next;
                            if ended(out.last().unwrap()) {
                                break;
                            }
                            continue;
                        }
                    } else {
                        self.retire(&c);
                    }
                }
            }
            // Reduce one real node.
            let ans = PageAnswers { fold: fold.clone() };
            let node = (*graph.reduce_with(&cur, &ans, &self.halt)).clone();
            feed(&mut fold, &cur.env, main_depth, &node);
            cur = node.next.clone();
            let end = ended(&node);
            out.push(node);
            self.stats.nodes += 1;
            if end {
                break;
            }
        }
        for c in chains.drain(..) {
            self.retire(&c);
        }
        self.last_envs = new_envs;
        (out, fold)
    }

    fn retire(&mut self, c: &Chain) {
        c.cancel.store(true, Ordering::Relaxed);
        let n = std::mem::take(&mut *c.nodes.lock().unwrap()).len();
        self.spec_nodes.fetch_sub(n, Ordering::Relaxed);
        self.stats.discarded += n;
    }

    fn start_chain(&mut self, graph: &Arc<Graph>, cur: &Thunk, b: Boundary, fold: &Fold) -> Option<Arc<Chain>> {
        if self.spec_nodes.load(Ordering::Relaxed) >= self.opts.max_spec_nodes {
            return None;
        }
        // The text the boundary is in: the root text at the input's bottom or above.
        let mut src = None;
        let mut i = &cur.input;
        while let Some(p) = &i.0 {
            if let Src::Text { text, data, eol, .. } = &p.0 {
                if *text == b.text {
                    src = Some(Src::Text { text: *text, data: data.clone(), pos: b.pos, line: Line::NewLine, line_end: b.pos, eol: *eol, ending: false });
                }
            }
            i = &p.1;
        }
        let src = src?;
        let env = self.last_envs.get(&b).cloned().unwrap_or_else(|| cur.env.clone());
        let chain = Arc::new(Chain {
            at: b,
            shape: shape(&env),
            nodes: Mutex::new(Vec::new()),
            done: AtomicBool::new(false),
            cancel: Arc::new(AtomicBool::new(false)),
        });
        self.stats.chains += 1;
        let c = chain.clone();
        let g = graph.clone();
        let ans = PageAnswers { fold: fold.clone() };
        let halt = self.halt.clone();
        let budget = self.spec_nodes.clone();
        let (max_nodes, chain_len) = (self.opts.max_spec_nodes, self.opts.chain_len);
        // Fire and forget: the chain publishes its nodes as it goes.
        drop(self.pool.spawn(move || {
            let mut t = Thunk { env, input: Input::default().push(src) };
            for _ in 0..chain_len {
                if c.cancel.load(Ordering::Relaxed) || halt.load(Ordering::Relaxed) || t.input.is_empty() {
                    break;
                }
                if budget.load(Ordering::Relaxed) >= max_nodes {
                    break;
                }
                let n = g.reduce_with(&t, &ans, &c.cancel);
                if c.cancel.load(Ordering::Relaxed) {
                    break;
                }
                let end = ended(&n);
                t = n.next.clone();
                budget.fetch_add(1, Ordering::Relaxed);
                c.nodes.lock().unwrap().push((*n).clone());
                if end {
                    break;
                }
            }
            c.done.store(true, Ordering::Release);
        }));
        Some(chain)
    }

    /// Adopts the chain's nodes onto the real thunk `cur` while every read
    /// holds; returns how many and the real thunk after them.
    fn adopt(&mut self, c: &Chain, cur: &Thunk, main_depth: u32, fold: &mut Fold, out: &mut Vec<Node>) -> (usize, Thunk) {
        c.cancel.store(true, Ordering::Relaxed);
        let mut nodes = std::mem::take(&mut *c.nodes.lock().unwrap());
        self.spec_nodes.fetch_sub(nodes.len(), Ordering::Relaxed);
        // The real input under the boundary's text.
        let base = cur.input.below();
        let mut real = cur.clone();
        let mut emitted_items = 0usize;
        let mut n = 0;
        for node in nodes.drain(..) {
            if !self.holds(&node, &real, fold, emitted_items) {
                break;
            }
            let next = Thunk { env: real.env.apply(&node.out.delta), input: rebase(&node.next.input, &base) };
            feed(fold, &real.env, main_depth, &node);
            emitted_items += node.out.emits.iter().filter(|e| matches!(e, Emit::Item(_))).count();
            let end = ended(&node);
            out.push(Node { next: next.clone(), ..node });
            real = next;
            n += 1;
            if end {
                break;
            }
        }
        self.stats.discarded += nodes.len();
        self.stats.adopted += n;
        self.stats.nodes += n;
        (n, real)
    }

    /// Every read `node` made equals what the real thunk would read.
    fn holds(&self, node: &Node, real: &Thunk, fold: &Fold, items_before: usize) -> bool {
        node.reads.iter().all(|(k, v)| match k {
            Key::Page(q) => *v == Val::Int(fold.quantity(*q)),
            // The tail of a list opened before the chain is not ours to see.
            Key::Tail => items_before > 0,
            // Spans are absolute in the text: the same line.
            Key::LineNo => true,
            _ => real.env.lookup(k) == *v,
        })
    }
}

impl Drop for Sched {
    fn drop(&mut self) {
        self.halt();
    }
}

/// The node ended the job: `\end` defined `Key::Ended`, or the input ran out.
fn ended(n: &Node) -> bool {
    n.next.input.is_empty() || n.out.delta.0.iter().any(|c| matches!(c, Change::Def { key: Key::Ended, .. }))
}

/// Feeds the page fold with the items the node put on the main vertical list.
fn feed(fold: &mut Fold, env: &Env, main_depth: u32, node: &Node) {
    let mut d = env.list_depth();
    for e in &node.out.emits {
        match e {
            Emit::Open(_) => d += 1,
            Emit::Close => d = d.saturating_sub(1),
            Emit::Item(it) if d == main_depth => {
                // GAP: a fire (`page::Fire`) is not applied here: the output
                // routine's node (`Op::Fire`, `output::fire`) is not wired yet.
                let (f, _fire) = crate::page::step(fold, it, &mut |k: &Key| env.lookup(k));
                *fold = f;
            }
            _ => {}
        }
    }
}
