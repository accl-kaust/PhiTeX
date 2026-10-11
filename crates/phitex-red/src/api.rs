//! E3: the build's API: `Build::new(options)`, `update(changes) -> Output`
//! (PDF/DVI bytes, pages changed, diagnostics), and the format cache.
//!
//! A build is one graph kept for the life of the document. The first
//! `update` reduces it cold from `pending(format env; main text)`; later
//! ones map the text changes onto the reduced chain (`edit::update`), hand
//! the new chain back to the graph, and run the loop-back fixpoint
//! (`fixpoint::build`), which re-reduces only what is stale. The pages that
//! come out are compared with the last ones: `changed_pages`.
//!
//! A format is not a dump file: it is the env after reducing the ini file
//! (`tex.ini`, `pdflatex.ini` …) to `\dump`, kept in process per
//! (engine, format) and shared by every build that loads it.
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::edit::{Changes, Chain, Origin};
use crate::env::{Change, Delta, Env};
use crate::graph::Graph;
use crate::output::Shipped;
use crate::term::{Emit, Input, TextId, Thunk};
use crate::val::{FileName, Key, Val};

/// TeX, e-TeX, or pdfTeX (e-TeX mode on).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Engine {
    Tex,
    Etex,
    Pdftex,
}

impl Engine {
    pub fn etex(self) -> bool {
        self != Engine::Tex
    }
    pub fn pdf(self) -> bool {
        self == Engine::Pdftex
    }
}

/// The format a job loads, or none (`-ini`: the main file is the whole job).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    Ini,
    Plain,
    Latex,
}

/// The file finder: a name as TeX asks for it → (the found name, its bytes).
pub type Finder = Arc<dyn Fn(&str) -> Option<(Arc<str>, Arc<[u8]>)> + Send + Sync>;

#[derive(Clone)]
pub struct Options {
    pub engine: Engine,
    pub format: Format,
    /// `\jobname`.
    pub job_name: Arc<str>,
    /// The main file's name (for errors and `\inputlineno`).
    pub main_name: Arc<str>,
    pub finder: Finder,
    /// Worker threads (0: the machine's).
    pub threads: usize,
    /// `Key::Given(n)` answers (seed, clock, creation date …).
    pub givens: BTreeMap<u8, Val>,
    /// Files the last build wrote (warm start across processes).
    pub previous: BTreeMap<FileName, Arc<str>>,
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("engine", &self.engine)
            .field("format", &self.format)
            .field("job_name", &self.job_name)
            .field("threads", &self.threads)
            .finish_non_exhaustive()
    }
}

/// What one `update` made.
#[derive(Clone, Debug, Default)]
pub struct Output {
    /// The PDF (pdfTeX), else empty.
    pub pdf: Vec<u8>,
    /// The DVI (TeX, e-TeX), else empty.
    pub dvi: Vec<u8>,
    pub pages: usize,
    /// Page indices (0-based) whose shipped box differs from the last update's.
    pub changed_pages: Vec<usize>,
    pub diags: Vec<Arc<crate::diag::Diag>>,
    /// Terminal/log text (`\message`, `\write16`…), in order.
    pub log: Vec<String>,
    /// Every file the job wrote (derived `.bbl`/`.ind` included).
    pub written: BTreeMap<FileName, Arc<str>>,
    pub rounds: u32,
    pub converged: bool,
    pub unsettled: Vec<FileName>,
    /// Nodes reduced (not kept from the last build) by this update.
    pub nodes_reduced: usize,
}

/// One change to the main file or to an included file.
#[derive(Clone, Debug)]
pub enum Edit {
    /// The main file's whole new text (the change is found by its common prefix/suffix).
    Main(Arc<str>),
    /// A byte change to the main file in the old text's offsets, and the whole new text.
    MainSplice { start: usize, old_len: usize, inserted: usize, data: Arc<str> },
    /// Another file's text now (an editor buffer), or `None`: as on disk again.
    File { name: Arc<str>, text: Option<Arc<str>> },
}

pub struct Build {
    opts: Options,
    graph: Graph,
    main: TextId,
    text: Arc<str>,
    /// Editor buffers that override the finder.
    buffers: Arc<RwLock<HashMap<Arc<str>, Arc<str>>>>,
    /// Pending change to the main text since the last update.
    changes: Changes,
    /// Included files whose buffer changed.
    files_changed: Vec<Key>,
    cold: bool,
    last_pages: Vec<Arc<Shipped>>,
    written: BTreeMap<FileName, Arc<str>>,
    pdf: crate::pdf::Writer,
}

// ------------------------------------------------------------------ formats

type FormatKey = (Engine, Format);

fn formats() -> &'static Mutex<HashMap<FormatKey, Env>> {
    static F: OnceLock<Mutex<HashMap<FormatKey, Env>>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The ini file fmtutil.cnf makes the format from.
pub fn ini_file(engine: Engine, format: Format) -> Option<&'static str> {
    Some(match (engine, format) {
        (_, Format::Ini) => return None,
        (Engine::Tex, Format::Plain) => "tex.ini",
        (Engine::Etex, Format::Plain) => "etex.ini",
        (Engine::Pdftex, Format::Plain) => "pdfetex.ini",
        (Engine::Pdftex, Format::Latex) => "pdflatex.ini",
        (_, Format::Latex) => "latex.ini",
    })
}

fn sched_opts(threads: usize) -> crate::sched::Options {
    let mut o = crate::sched::Options::default();
    if threads > 0 {
        o.threads = threads;
        o.speculate = threads > 1;
        o.max_chains = threads.saturating_sub(1).max(1);
    }
    o
}

/// The finder with editor buffers in front of it.
fn layered(finder: Finder, buffers: Arc<RwLock<HashMap<Arc<str>, Arc<str>>>>) -> Finder {
    Arc::new(move |name: &str| {
        if let Some(t) = buffers.read().unwrap().get(name) {
            return Some((Arc::from(name), Arc::from(t.as_bytes())));
        }
        finder(name)
    })
}

fn text_thunk(env: Env, text: TextId, data: Arc<str>) -> Thunk {
    Thunk { env, input: Input::default().push(crate::input::open(text, data)) }
}

/// The env a job starts in: iniTeX's, or the format's (reduced once per process).
pub fn format_env(opts: &Options) -> Result<Env, String> {
    let ini_env = crate::graph::initial_env(opts.engine.etex(), opts.engine.pdf());
    let Some(ini) = ini_file(opts.engine, opts.format) else { return Ok(ini_env) };
    let key = (opts.engine, opts.format);
    if let Some(e) = formats().lock().unwrap().get(&key) {
        return Ok(e.clone());
    }
    let (_, bytes) = (opts.finder)(ini).ok_or_else(|| format!("{ini}: not found"))?;
    let data: Arc<str> = Arc::from(String::from_utf8_lossy(&bytes).as_ref());
    let mut g = Graph::new(opts.finder.clone(), opts.engine.etex(), opts.engine.pdf(), sched_opts(opts.threads));
    let id = g.add_text(Arc::from(ini), data.clone());
    g.set_root(text_thunk(ini_env, id, data));
    let fopts = crate::fixpoint::Options {
        job_name: Arc::from(ini.trim_end_matches(".ini")),
        givens: opts.givens.clone(),
        previous: BTreeMap::new(),
    };
    crate::fixpoint::build(&fopts, &mut g);
    let end = g.end_env();
    if end.lookup(&Key::Ended) == Val::Void {
        return Err(format!("{ini}: the format ended without \\dump"));
    }
    // the job starts where \dump left off, not ended; \dump leaves group 0 only
    let env = end.apply(&Delta(vec![Change::Def { key: Key::Ended, val: Val::Void, global: true }]));
    formats().lock().unwrap().insert(key, env.clone());
    Ok(env)
}

// -------------------------------------------------------------------- build

impl Build {
    pub fn new(opts: Options, main_text: Arc<str>) -> Result<Build, String> {
        let buffers: Arc<RwLock<HashMap<Arc<str>, Arc<str>>>> = Arc::default();
        let finder = layered(opts.finder.clone(), buffers.clone());
        let opts = Options { finder, ..opts };
        let env = format_env(&opts)?;
        let mut graph = Graph::new(opts.finder.clone(), opts.engine.etex(), opts.engine.pdf(), sched_opts(opts.threads));
        let main = graph.add_text(opts.main_name.clone(), main_text.clone());
        graph.set_root(text_thunk(env, main, main_text.clone()));
        Ok(Build {
            written: opts.previous.clone(),
            opts,
            graph,
            main,
            text: main_text,
            buffers,
            changes: Changes::default(),
            files_changed: Vec::new(),
            cold: true,
            last_pages: Vec::new(),
            pdf: crate::pdf::Writer::default(),
        })
    }

    pub fn options(&self) -> &Options {
        &self.opts
    }

    pub fn text(&self) -> &Arc<str> {
        &self.text
    }

    fn change_main(&mut self, start: usize, old_len: usize, inserted: usize, data: Arc<str>) {
        // successive edits before an update: the last text wins, as one change
        // covering all of them in the text the chain was reduced from
        if self.changes.edits.is_empty() {
            self.changes.add(self.main, start, old_len, inserted, data.clone());
        } else {
            // several edits before an update: one change from the reduced text
            let (s, o, i) = diff(&self.reduced_text(), &data);
            self.changes = Changes::default();
            self.changes.add(self.main, s, o, i, data.clone());
        }
        self.graph.set_text(self.main, data.clone());
        self.text = data;
    }

    /// The main text the chain was last reduced from.
    fn reduced_text(&self) -> Arc<str> {
        self.graph.reduced_text(self.main).unwrap_or_else(|| self.text.clone())
    }

    /// Records changes; they take effect at the next `update`.
    pub fn edit(&mut self, e: Edit) {
        match e {
            Edit::Main(data) => {
                if data == self.text {
                    return;
                }
                let (s, o, i) = diff(&self.text, &data);
                self.change_main(s, o, i, data);
            }
            Edit::MainSplice { start, old_len, inserted, data } => self.change_main(start, old_len, inserted, data),
            Edit::File { name, text } => {
                let mut b = self.buffers.write().unwrap();
                let old = match &text {
                    Some(t) => b.insert(name.clone(), t.clone()),
                    None => b.remove(&name),
                };
                if old != text {
                    self.files_changed.push(Key::File(name));
                }
            }
        }
    }

    /// Applies `changes` and builds to the fixpoint.
    pub fn update(&mut self, changes: Vec<Edit>) -> Output {
        for e in changes {
            self.edit(e);
        }
        let mut reduced_before = 0usize;
        if !self.cold {
            if !self.changes.edits.is_empty() {
                let old: Chain = self.graph.chain();
                let (chain, report) = crate::edit::update(&self.graph, &old, &self.changes);
                reduced_before = report.origin.iter().filter(|o| !matches!(o, Origin::Kept(_))).count();
                self.graph.set_chain(chain);
            }
            if !self.files_changed.is_empty() {
                let keys = std::mem::take(&mut self.files_changed);
                crate::edit::invalidate_reads(&mut self.graph, &keys);
            }
        }
        self.changes = Changes::default();
        self.files_changed.clear();
        let count_before = if self.cold { 0 } else { self.graph.nodes_reduced() };

        let fopts = crate::fixpoint::Options {
            job_name: self.opts.job_name.clone(),
            givens: self.opts.givens.clone(),
            previous: self.written.clone(),
        };
        let conv = crate::fixpoint::build(&fopts, &mut self.graph);
        let nodes_reduced = reduced_before + self.graph.nodes_reduced().saturating_sub(count_before);
        self.cold = false;

        let mut diags = Vec::new();
        let mut log = Vec::new();
        for node in self.graph.nodes() {
            for e in &node.out.emits {
                match e {
                    Emit::Diag(d) => diags.push(d.clone()),
                    Emit::Write { stream, text } if *stream >= 16 => log.push(text.to_string()),
                    _ => {}
                }
            }
        }

        let changed_pages: Vec<usize> = conv
            .pages
            .iter()
            .enumerate()
            .filter(|(i, p)| self.last_pages.get(*i).is_none_or(|q| !Arc::ptr_eq(p, q) && **p != **q))
            .map(|(i, _)| i)
            .collect();
        let (pdf, dvi) = if conv.pages.is_empty() {
            (Vec::new(), Vec::new())
        } else if self.opts.engine.pdf() {
            (self.pdf.write(&conv.pages), Vec::new())
        } else {
            let comment = format!(" phitex-red output {}", self.opts.job_name);
            (Vec::new(), crate::dvi::write(&conv.pages, &comment))
        };
        self.last_pages = conv.pages.clone();
        self.written = conv.written.clone();
        Output {
            pdf,
            dvi,
            pages: conv.pages.len(),
            changed_pages,
            diags,
            log,
            written: conv.written,
            rounds: conv.rounds,
            converged: conv.converged,
            unsettled: conv.unsettled,
            nodes_reduced,
        }
    }
}

/// The change from `a` to `b` as (start, old length, inserted length), on char boundaries.
pub fn diff(a: &str, b: &str) -> (usize, usize, usize) {
    let (x, y) = (a.as_bytes(), b.as_bytes());
    let mut p = x.iter().zip(y).take_while(|(m, n)| m == n).count();
    while !a.is_char_boundary(p) || !b.is_char_boundary(p) {
        p -= 1;
    }
    let max = x.len().min(y.len()) - p;
    let mut s = x.iter().rev().zip(y.iter().rev()).take(max).take_while(|(m, n)| m == n).count();
    while !a.is_char_boundary(x.len() - s) || !b.is_char_boundary(y.len() - s) {
        s -= 1;
    }
    (p, x.len() - s - p, y.len() - s - p)
}

#[cfg(test)]
mod tests {
    use super::diff;
    #[test]
    fn diffs() {
        assert_eq!(diff("abc", "abc"), (3, 0, 0));
        assert_eq!(diff("abc", "aXc"), (1, 1, 1));
        assert_eq!(diff("aa", "aaa"), (2, 0, 1));
        assert_eq!(diff("é", "è"), (0, 2, 2));
    }
}
