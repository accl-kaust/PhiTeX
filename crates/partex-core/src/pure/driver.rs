//! The build on the φ core: the job's unfold, a cold build, a rebuild
//! after the files changed, and what the build made (its effects in
//! program order, its history).

use alloc::sync::Arc;
use alloc::vec::Vec;

use phi::{Graph, NodeId, Seq, Ver};

use crate::effects::Effect;
use crate::host::{FileKind, Host, NoHost};
use crate::run::Step as Run;
use crate::tex::Tex;

use super::files::{FileDoc, all, key, key_hash, set_doc};
use super::lang::{Engine, OUTPUT, Op, Stats, TexLang, Val, install, uninstall};
use super::state::PState;
use super::tracker::PureTracker;

/// A pure SSA build.
pub struct Build<H: Host + 'static> {
    pub g: Graph<TexLang<H>>,
    doc: NodeId,
    engine: Option<Engine<H>>,
    /// The effects of the job's start (before its first command).
    pub fx0: Vec<Effect>,
    /// The job ended before its first command.
    done0: Option<i32>,
}

/// What a refresh of the files found.
#[derive(Default, Debug, Clone, Copy)]
pub struct Refresh {
    pub changed: usize,
    pub removed: usize,
}

impl<H: Host + 'static> Build<H> {
    /// A build of the job `command_line`: the engine set up and run to
    /// its first command (the format loaded), the graph made. The files
    /// the job reads are found by its steps.
    pub fn new(mut tex: Tex<H, PureTracker>, command_line: &[u8], workers: usize) -> Self {
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
        tex.tracker.file_ends.borrow_mut().clear();
        let mut g: Graph<TexLang<H>> = Graph::new();
        g.cfg.workers = workers.max(1);
        // (the job's own input is the terminal's line, in the state: the
        // unfold reads no elements; its files are calls)
        let input = g.input(Val::Lines(Seq::new(), Ver::of(&0u8)));
        // (a file the terminal's line named, opened before the first
        // command (§1337): a source, which the job's first step calls)
        let mut first = None;
        for l in tex.tracker.loads.borrow_mut().drain(..) {
            let (Some((name, bytes)), true) = (&l.found, l.lines) else {
                continue;
            };
            let j = tex.in_open;
            if j > 0
                && tex.input_file[j]
                    .as_ref()
                    .is_some_and(|f| Arc::ptr_eq(&f.data, bytes))
            {
                let k = key(&l.name);
                let h = key_hash(&k);
                let d = FileDoc::new(&l.name, name, bytes.clone(), None);
                g.source(&k, d.value.clone());
                set_doc(h, Some(d));
                first = Some((h, u16::try_from(j).expect("input levels fit u16")));
            }
        }
        let st = Arc::new(st);
        let init = g.input(match first {
            Some((h, j)) => Val::Enter(st, Ver(v), h, j),
            None => Val::State(st, Ver(v)),
        });
        let doc = g.unfold(Op::Main, input, init, &[]);
        let _ = g.chain_read(OUTPUT);
        Build {
            g,
            doc,
            engine: Some(Engine {
                tex,
                base,
                at: Some(Ver(v)),
                stats: Stats::default(),
                defined: std::collections::HashSet::new(),
                last: None,
                last_defs: std::collections::HashMap::new(),
            }),
            fx0,
            done0,
        }
    }

    /// Each file the build has, as the host has it now: a changed one's
    /// source set (its lines' identities kept where the lines are the
    /// same), a gone one's removed. The engine's arrays are a cache no
    /// longer: the next step makes its frontier afresh.
    pub fn refresh(&mut self) -> Refresh {
        let mut rep = Refresh::default();
        let e = self.engine.as_mut().expect("pure SSA: the engine");
        for (h, d) in all() {
            let k = key(&d.asked);
            debug_assert_eq!(key_hash(&k), h);
            match e.tex.host.read_file(&d.asked, FileKind::Tex) {
                Some(f) if f.contents[..] == d.bytes[..] && f.name[..] == d.name[..] => {}
                Some(f) => {
                    let nd = FileDoc::new(&d.asked, &f.name, f.contents.clone(), Some(&d));
                    self.g.source(&k, nd.value.clone());
                    set_doc(h, Some(nd));
                    rep.changed += 1;
                }
                None => {
                    self.g.remove_source(&k);
                    set_doc(h, None);
                    rep.removed += 1;
                }
            }
        }
        e.at = None;
        e.last = None;
        e.last_defs.clear();
        rep
    }

    /// Run the graph to quiescence.
    pub fn run(&mut self) -> phi::Report {
        install(self.engine.take().expect("pure SSA: the engine"));
        let rep = self.g.run();
        self.engine = uninstall();
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
