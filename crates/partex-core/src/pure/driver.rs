//! The build on the φ core: the graph over the document's lines, a cold
//! build, an edit and a rebuild, and what the build made (its effects in
//! program order, its history).

use alloc::sync::Arc;
use alloc::vec::Vec;
use std::collections::HashMap;

use phi::{ElemId, Graph, NodeId, Seq};

use crate::effects::Effect;
use crate::host::{Host, NoHost};
use crate::run::Step as Run;
use crate::tex::Tex;

use super::lang::{DOC, Doc, Engine, OUTPUT, Op, Stats, TexLang, Val, install, uninstall};
use super::state::PState;
use super::tracker::PureTracker;

/// A pure SSA build.
pub struct Build<H: Host + 'static> {
    pub g: Graph<TexLang<H>>,
    input: NodeId,
    doc: NodeId,
    out: NodeId,
    engine: Option<Engine<H>>,
    /// The main file's lines: identity and bytes.
    lines: Vec<(ElemId, Arc<[u8]>)>,
    /// The next identity a new line takes.
    next_id: u64,
    name: Option<Arc<[u8]>>,
    /// The effects of the job's start (before its first command).
    pub fx0: Vec<Effect>,
    /// The job ended before its first command.
    done0: Option<i32>,
}

impl<H: Host + 'static> Build<H> {
    /// A build of the job `command_line` whose main file holds `main`
    /// (the core's input): the engine set up and run to its first
    /// command (the format loaded), the graph made.
    pub fn new(mut tex: Tex<H, PureTracker>, command_line: &[u8], main: &[u8], workers: usize) -> Self {
        // (the files are linked from the steps' effects)
        tex.set_effects(true);
        // (boxes carry versions, made when each becomes a shared value)
        partex_engine::node::VERSIONS.store(true, core::sync::atomic::Ordering::Relaxed);
        tex.remake_constant_lists();
        tex.skip.epoch += 1;
        tex.version_tables();
        let r = tex.start(command_line);
        let fx0 = tex.take_effects();
        let done0 = match r {
            Run::Checkpoint => None,
            Run::Finished(h) => Some(h),
        };
        let mut base = tex.fork_with(NoHost, PureTracker::default());
        // (its tables flat, to be read: a checkpoint left them frozen)
        base.thaw();
        let (st, v) = PState::of(&mut tex, b"", done0);
        let mut g: Graph<TexLang<H>> = Graph::new();
        g.cfg.workers = workers.max(1);
        let mut b = Build {
            g,
            input: NodeId(0),
            doc: NodeId(0),
            out: NodeId(0),
            engine: Some(Engine {
                tex,
                base,
                at: Some(phi::Ver(v)),
                stats: Stats::default(),
                defined: std::collections::HashSet::new(),
                last: None,
                last_defs: std::collections::HashMap::new(),
            }),
            lines: Vec::new(),
            next_id: 1,
            name: None,
            fx0,
            done0,
        };
        let seq = b.set_lines(main);
        b.input = b.g.input(Val::Lines(seq));
        let init = b.g.input(Val::State(Arc::new(st), phi::Ver(v)));
        b.doc = b.g.unfold(Op::Main, b.input, init, &[]);
        b.out = b.g.chain_read(OUTPUT);
        b
    }

    /// The main file's lines as `main` has them: the lines kept from the
    /// last text keep their identities (a common prefix and suffix), the
    /// others take new ones between them.
    fn set_lines(&mut self, main: &[u8]) -> Seq<Val> {
        let ranges = Doc::lines(main);
        let new: Vec<Arc<[u8]>> = ranges.iter().map(|r| Arc::from(&main[r.clone()])).collect();
        let old = core::mem::take(&mut self.lines);
        let pre = old
            .iter()
            .zip(&new)
            .take_while(|(a, b)| a.1[..] == b[..])
            .count();
        let suf = old[pre..]
            .iter()
            .rev()
            .zip(new[pre..].iter().rev())
            .take_while(|(a, b)| a.1[..] == b[..])
            .count();
        let mut lines: Vec<(ElemId, Arc<[u8]>)> = Vec::with_capacity(new.len());
        lines.extend(old[..pre].iter().cloned());
        for l in &new[pre..new.len() - suf] {
            lines.push((ElemId(self.next_id << 8), l.clone()));
            self.next_id += 1;
        }
        lines.extend(old[old.len() - suf..].iter().cloned());
        self.lines = lines;
        let mut starts = Vec::with_capacity(ranges.len() + 1);
        let mut index = HashMap::with_capacity(ranges.len());
        for (i, r) in ranges.iter().enumerate() {
            starts.push(r.start);
            index.insert(self.lines[i].0, i);
        }
        starts.push(main.len());
        let doc = Doc {
            name: self.name.clone().unwrap_or_else(|| Arc::from(&b""[..])),
            name_cell: std::sync::OnceLock::new(),
            bytes: Arc::from(main),
            starts,
            index,
        };
        if let Some(n) = &self.name {
            let _ = doc.name_cell.set(n.clone());
        }
        *DOC.write().expect("pure SSA: the document") = Some(Arc::new(doc));
        Seq::from_vec(
            self.lines
                .iter()
                .map(|(id, l)| (*id, Val::Line(l.clone(), phi::Ver::of(&l[..]))))
                .collect(),
        )
    }

    /// The main file edited to `main`.
    pub fn edit(&mut self, main: &[u8]) {
        let seq = self.set_lines(main);
        self.g.set(self.input, Val::Lines(seq));
        if let Some(e) = self.engine.as_mut() {
            // (the engine's arrays are a cache: its state is checked again)
            e.at = None;
        }
    }

    /// Run the graph to quiescence.
    pub fn run(&mut self) -> phi::Report {
        install(self.engine.take().expect("pure SSA: the engine"));
        let rep = self.g.run();
        self.engine = uninstall();
        if self.name.is_none() {
            self.name = DOC
                .read()
                .ok()
                .and_then(|d| d.as_ref().and_then(|d| d.name_cell.get().cloned()));
        }
        rep
    }

    /// The engine (its host, for the link).
    pub fn engine(&mut self) -> &mut Engine<H> {
        self.engine.as_mut().expect("pure SSA: the engine")
    }

    /// Every effect of the job in program order, by step.
    #[must_use]
    pub fn effects(&self) -> Vec<Arc<Vec<Effect>>> {
        let mut out = Vec::new();
        out.push(Arc::new(self.fx0.clone()));
        for v in self.g.chain(OUTPUT) {
            if let Val::Fx(x, _) = v {
                out.push(x);
            }
        }
        out
    }

    /// The job's history, once it ended.
    #[must_use]
    pub fn history(&self) -> Option<i32> {
        if let Some(h) = self.done0 {
            return Some(h);
        }
        match self.g.value(self.doc) {
            Val::Done(h) => Some(*h),
            _ => None,
        }
    }
}
