//! The static document layer (DESIGN 4.3.6): what a LaTeX document's
//! structure is, read from its source without running TeX. The outline,
//! labels, references, citations, the file graph, environments and the
//! document's own definitions, kept up to date as the document is edited.
//!
//! **Data.** A [`Project`] keeps a [`phitex_syntax::Tree`] per file and,
//! beside each paragraph, its [`ParaFacts`]: the facts the paragraph
//! states ([`Fact`]: a heading, a label, an `\input`, …, at an offset in
//! the paragraph), the state reading leaves at its end ([`State`]:
//! skipping an `\iffalse`, or past `\endinput`), and the macros it calls.
//! A paragraph's facts are a function of its text, its entry state and
//! the table of the document's definitions (`defs.rs`), and nothing else:
//! not its position, not the files around it.
//!
//! **Edits.** [`Project::edit`] replaces a byte range of a file: the tree
//! reparses the paragraphs it touches ([`phitex_syntax::Tree::edit`]),
//! and only those are read again; then the next ones while the state
//! they begin in changed; then, if a definition changed, the paragraphs
//! that call it (or a macro whose code calls it); then the files a new
//! `\input` names are loaded. Facts are kept by paragraph, so every other
//! paragraph keeps its facts, keyed by its [`ParaId`].
//!
//! **Views.** The whole document's views ([`Views`]: the outline with
//! LaTeX's numbers, the table of contents as LaTeX writes it, labels with
//! the duplicates, references with the unresolved ones, citations against
//! the `.bib` keys, the file graph, environments that do not nest) are a
//! fold over the paragraphs' facts in reading order, descending into an
//! `\input` where it is ([`Project::views`]).
//!
//! **Guards.** The layer reads `\section`, `\label`, `\ref`, `\cite`,
//! `\input`, `\begin`, … as LaTeX defines them (`known.rs`). A document
//! that defines one of them, or loads a package known to change one,
//! breaks that construct's guard: the views report it, and mark each fact
//! read through it unguarded. What the facts assume is a list of
//! control sequences ([`Project::assumptions`]), each with the meaning it
//! must have (LaTeX's, or the document's definition at a place), which a
//! build's records can confirm or break ([`Project::confirm`]).

mod bib;
pub mod compare;
mod defs;
mod known;
mod outline;
mod report;
mod scan;
pub mod text;
mod views;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

use phitex_syntax::{ParaId, Splice, Tree};

pub use bib::Bib;
pub use defs::{Args, Def, Spec};
pub use known::{AssetKind, CounterOp, Definer, InputKind, LEVELS, Matter};
pub use outline::{Entry, GlyphRef, Outline};
pub use report::{Timing, json, report};
pub use views::{
    AssetRef, Assumption, BibRef, Broken, EnvProblem, FileRef, GuardState, Heading, Keyed, Loc,
    Meant, TocLine, Verdict, Views,
};

/// A control sequence's name (without its backslash), or an
/// environment's.
pub type Name = Rc<str>;

/// A fast hash for the layer's short keys (rustc's `FxHash`): nothing
/// here needs protection against keys chosen to collide.
#[derive(Clone, Copy, Default)]
pub struct Fx(u64);

impl Fx {
    fn add(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

impl std::hash::Hasher for Fx {
    fn write(&mut self, bytes: &[u8]) {
        let (chunks, rest) = bytes.as_chunks::<8>();
        for c in chunks {
            self.add(u64::from_le_bytes(*c));
        }
        if !rest.is_empty() {
            let mut b = [0u8; 8];
            b[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(b));
        }
    }

    fn write_u8(&mut self, i: u8) {
        self.add(u64::from(i));
    }

    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

pub type FxMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<Fx>>;
pub type FxSet<K> = std::collections::HashSet<K, std::hash::BuildHasherDefault<Fx>>;

/// A file of the project, by the order it was loaded in.
pub type FileId = usize;

/// Where the layer reads files from: a file system, an editor's buffers.
/// Paths are relative to the main file's directory (as TeX resolves them,
/// whatever file names them), with `/`.
pub trait Files {
    /// The file's text (`None`: there is none, or it is not text).
    fn read(&self, path: &str) -> Option<String>;
    /// Whether the file exists.
    fn exists(&self, path: &str) -> bool;
}

/// What reading leaves at a paragraph's end, for the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// Inside a skipped `\iffalse`, this many conditionals deep.
    pub skip: u32,
    /// Past `\endinput` (or `\end{document}`): the file is not read on.
    pub ended: bool,
}

/// A fact a paragraph states.
#[derive(Clone, Debug, PartialEq)]
pub struct Fact {
    /// Its offset in the paragraph: its command's backslash, or the call
    /// of the document's macro it came from.
    pub at: u32,
    /// The control sequence it was read through (`section`, `label`, …):
    /// the meaning it assumes.
    pub cs: &'static str,
    /// The document's macros expanded to reach it, outermost first.
    pub via: Option<Rc<[Name]>>,
    pub what: What,
}

/// What a fact says.
#[derive(Clone, Debug, PartialEq)]
pub enum What {
    /// `\section*[short]{title}`: its level (`\part` −1 … `\subparagraph`
    /// 5), where its title ends in the paragraph, and where the title's
    /// text begins there (`None`: read through a macro, not in the
    /// paragraph's text).
    Heading {
        level: i8,
        star: bool,
        short: Option<String>,
        title: String,
        end: u32,
        title_at: Option<u32>,
    },
    /// `\addcontentsline{list}{level}{title}`.
    Toc {
        list: String,
        level: String,
        title: String,
    },
    Label(String),
    Ref(Vec<String>),
    Cite {
        keys: Vec<String>,
        nocite: bool,
    },
    /// A file read as TeX, as named; `sub`: relative to the directory of
    /// the file that names it (`\subimport`), not the main file's.
    Input {
        path: String,
        how: InputKind,
        sub: bool,
    },
    IncludeOnly(Vec<String>),
    /// `.bib` files, as BibTeX or biber looks for them.
    Bib(Vec<String>),
    /// A file read as data (`\includegraphics`), as named.
    Asset {
        path: String,
        how: AssetKind,
    },
    GraphicsPath(Vec<String>),
    /// A `filecontents` environment: the file it writes, and its text.
    Embedded {
        path: String,
        text: String,
    },
    Begin(String),
    End(String),
    Define(Rc<Def>),
    /// A definition of a name the layer cannot know
    /// (`\expandafter\def\csname…\endcsname`).
    DynamicDefine,
    /// A sectioning counter changed (`value`: if the layer can tell).
    Counter {
        name: String,
        op: CounterOp,
        value: Option<i64>,
    },
    Matter(Matter),
    Class(String),
    Packages(Vec<String>),
    /// `\IfFileExists{test}{then}{other}`: the facts of each branch.
    If {
        test: String,
        then: Vec<Fact>,
        other: Vec<Fact>,
    },
}

/// A paragraph's facts.
#[derive(Clone, Debug, PartialEq)]
pub struct ParaFacts {
    pub id: ParaId,
    /// The state it was read from, and the one it leaves.
    pub entry: State,
    pub exit: State,
    pub facts: Vec<Fact>,
    /// The macros (and `{environments}`) it calls that the layer does not
    /// know, sorted: those the document's definitions may give a meaning.
    pub calls: Vec<Name>,
    /// Its newlines (line numbers are counted from them).
    pub newlines: u32,
    /// When it was read (a definition changed later makes it stale).
    epoch: u64,
}

impl ParaFacts {
    /// Its own definitions: those its text makes, not an expansion's.
    fn defs(&self) -> Vec<Rc<Def>> {
        fn walk(facts: &[Fact], out: &mut Vec<Rc<Def>>) {
            for f in facts.iter().filter(|f| f.via.is_none()) {
                match &f.what {
                    What::Define(d) => out.push(d.clone()),
                    What::If { then, other, .. } => {
                        walk(then, out);
                        walk(other, out);
                    }
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.facts, &mut out);
        out
    }

    /// The files it names, to be loaded.
    fn refs(&self) -> Vec<Ref> {
        fn walk(facts: &[Fact], out: &mut Vec<Ref>) {
            for f in facts {
                match &f.what {
                    What::Input { path, how, sub } => out.push(Ref::Tex {
                        path: path.clone(),
                        how: *how,
                        sub: *sub,
                    }),
                    What::Bib(files) => out.extend(files.iter().cloned().map(Ref::Bib)),
                    What::Class(c) => out.push(Ref::Local(format!("{c}.cls"))),
                    What::Packages(p) => {
                        out.extend(p.iter().map(|p| Ref::Local(format!("{p}.sty"))));
                    }
                    What::If { then, other, .. } => {
                        walk(then, out);
                        walk(other, out);
                    }
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.facts, &mut out);
        out
    }
}

/// A file a paragraph names.
enum Ref {
    Tex {
        path: String,
        how: InputKind,
        sub: bool,
    },
    Bib(String),
    /// A class or package of the document's own, if there is one.
    Local(String),
}

/// A file of the project: its tree, and its paragraphs' facts.
pub struct File {
    pub path: String,
    tree: Tree,
    paras: Vec<ParaFacts>,
}

impl File {
    /// Its length in bytes.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.tree.len()
    }
}

/// A build's verdict on what the layer assumed of a control sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirmed {
    /// Its meaning at every use was the one assumed.
    Kept,
    /// It was not: why.
    Broken(String),
}

/// What an edit did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Update {
    /// The paragraphs the tree replaced.
    pub splice: Splice,
    /// The paragraphs read again (the splice's, those after it whose
    /// state changed, the callers of a changed definition).
    pub read: usize,
    /// The files loaded.
    pub loaded: usize,
    /// The definitions whose meaning changed.
    pub defs_changed: usize,
}

/// Counts of what the layer did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Paragraphs read (each time).
    pub read: usize,
    /// Definitions whose meaning changed.
    pub defs_changed: usize,
}

/// A document: its files' trees and facts, and the table of its
/// definitions.
pub struct Project {
    io: Rc<dyn Files>,
    main: FileId,
    files: Vec<File>,
    by_path: FxMap<String, FileId>,
    /// The files tried and not found.
    missing: BTreeSet<String>,
    defs: defs::Defs,
    bibs: FxMap<String, Option<Bib>>,
    exists: RefCell<FxMap<String, bool>>,
    verdicts: BTreeMap<String, Confirmed>,
    /// A clock: one tick per change of the definitions.
    epoch: u64,
    /// When each call's meaning last changed (by `defs::call_key`).
    changed_at: FxMap<Name, u64>,
    /// A definition changed since the paragraphs that call it were read.
    dirty: bool,
    stats: Stats,
}

impl Project {
    /// The document whose main file is `main`, read from `io`: every file
    /// it reaches is loaded and read, in TeX's reading order (an
    /// `\input`'s file where the `\input` is), then the paragraphs that
    /// called a macro defined after them are read again.
    pub fn open(main: &str, io: Rc<dyn Files>) -> Project {
        let mut p = Project {
            io,
            main: 0,
            files: Vec::new(),
            by_path: FxMap::default(),
            missing: BTreeSet::new(),
            defs: defs::Defs::default(),
            bibs: FxMap::default(),
            exists: RefCell::new(FxMap::default()),
            verdicts: BTreeMap::new(),
            epoch: 0,
            changed_at: FxMap::default(),
            dirty: false,
            stats: Stats::default(),
        };
        let main = text::normalize(main);
        if p.load(&main).is_none() {
            // (no main file: an empty one, so that edits can make it)
            p.by_path.insert(main.clone(), 0);
            p.files.push(File {
                path: main,
                tree: Tree::default(),
                paras: Vec::new(),
            });
        }
        p.settle();
        p
    }

    #[must_use]
    pub fn main(&self) -> FileId {
        self.main
    }

    #[must_use]
    pub fn file_id(&self, path: &str) -> Option<FileId> {
        self.by_path.get(&text::normalize(path)).copied()
    }

    /// A file's path.
    #[must_use]
    pub fn path(&self, id: FileId) -> &str {
        &self.files[id].path
    }

    /// A file's text.
    #[must_use]
    pub fn text(&self, id: FileId) -> String {
        self.files[id].tree.text()
    }

    #[must_use]
    pub fn tree(&self, id: FileId) -> &Tree {
        &self.files[id].tree
    }

    /// A file's paragraphs' facts, in order (beside the tree's
    /// paragraphs).
    #[must_use]
    pub fn paras(&self, id: FileId) -> &[ParaFacts] {
        &self.files[id].paras
    }

    /// The files loaded, by id.
    pub fn files(&self) -> impl Iterator<Item = (FileId, &File)> {
        self.files.iter().enumerate()
    }

    /// The files tried and not found.
    pub fn missing(&self) -> impl Iterator<Item = &str> {
        self.missing.iter().map(String::as_str)
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// The keys of a `.bib` file loaded (`None`: not found).
    #[must_use]
    pub fn bib(&self, path: &str) -> Option<&Bib> {
        self.bibs.get(path).and_then(Option::as_ref)
    }

    /// The document's definitions of a name (an environment's if `env`):
    /// where each is.
    pub fn definitions(&self, env: bool, name: &str) -> impl Iterator<Item = (FileId, &Def)> {
        self.defs
            .placed(env, name)
            .iter()
            .map(|p| (p.file, &*p.def))
    }

    /// Whether a file exists (loaded, or as the host says).
    #[must_use]
    pub fn exists(&self, path: &str) -> bool {
        if self.by_path.contains_key(path) {
            return true;
        }
        if self.missing.contains(path) {
            return false;
        }
        if let Some(&e) = self.exists.borrow().get(path) {
            return e;
        }
        let e = self.io.exists(path);
        self.exists.borrow_mut().insert(path.to_owned(), e);
        e
    }

    /// A file appeared, disappeared or changed outside the editor: what
    /// the layer knows of it is forgotten, and a loaded file read again.
    pub fn invalidate(&mut self, path: &str) {
        let path = text::normalize(path);
        self.missing.remove(&path);
        self.exists.borrow_mut().remove(&path);
        self.bibs.remove(&path);
        if let Some(id) = self.file_id(&path) {
            let len = self.files[id].tree.len();
            let text = self.io.read(&path).unwrap_or_default();
            self.edit(id, 0, len, &text);
        } else {
            // (an `\input` of it may now find it)
            self.epoch += 1;
            for f in 0..self.files.len() {
                for i in 0..self.files[f].paras.len() {
                    let refs = self.files[f].paras[i].refs();
                    self.follow(f, &refs);
                }
            }
            self.settle();
        }
    }

    /// Replace `old_len` bytes at `start` of a file with `text`.
    ///
    /// # Panics
    ///
    /// If the bytes are not within the file, on character boundaries.
    pub fn edit(&mut self, file: FileId, start: usize, old_len: usize, text: &str) -> Update {
        let read0 = self.stats.read;
        let defs0 = self.stats.defs_changed;
        let loaded0 = self.files.len();
        let splice = self.files[file].tree.edit(start, old_len, text);
        let at = splice.at;
        let old: Vec<ParaFacts> = self.files[file]
            .paras
            .drain(at..at + splice.removed)
            .collect();
        for o in &old {
            self.apply_defs(file, o.id, &o.defs(), &[]);
        }
        let mut state = if at == 0 {
            State::default()
        } else {
            self.files[file].paras[at - 1].exit
        };
        let mut new = Vec::with_capacity(splice.inserted);
        for k in 0..splice.inserted {
            let pf = self.read_para(file, at + k, state);
            state = pf.exit;
            new.push(pf);
        }
        let mut refs = Vec::new();
        for pf in &new {
            self.apply_defs(file, pf.id, &[], &pf.defs());
            refs.extend(pf.refs());
        }
        self.files[file].paras.splice(at..at, new);
        // (the state the edit left, carried on while it differs)
        let mut j = at + splice.inserted;
        while j < self.files[file].paras.len() && self.files[file].paras[j].entry != state {
            let pf = self.read_para(file, j, state);
            state = pf.exit;
            refs.extend(self.replace_para(file, j, pf));
            j += 1;
        }
        self.follow(file, &refs);
        self.settle();
        Update {
            splice,
            read: self.stats.read - read0,
            loaded: self.files.len() - loaded0,
            defs_changed: self.stats.defs_changed - defs0,
        }
    }

    /// A build's verdict on the meaning the layer assumed of `cs` (a
    /// control sequence's name, or `{env}`): the views mark the facts read
    /// through it unguarded if it is broken.
    pub fn confirm(&mut self, cs: &str, verdict: Confirmed) {
        self.verdicts.insert(cs.to_owned(), verdict);
    }

    /// The verdicts given.
    pub(crate) fn verdict(&self, cs: &str) -> Option<&Confirmed> {
        self.verdicts.get(cs)
    }

    /// Load `path` (normalized) and read it, if it exists and is not
    /// loaded yet.
    fn load(&mut self, path: &str) -> Option<FileId> {
        if let Some(&id) = self.by_path.get(path) {
            return Some(id);
        }
        if self.missing.contains(path) {
            return None;
        }
        let Some(src) = self.io.read(path) else {
            self.missing.insert(path.to_owned());
            return None;
        };
        let tree = Tree::parse(&src);
        let id = self.files.len();
        let n = tree.paras().len();
        self.files.push(File {
            path: path.to_owned(),
            tree,
            paras: Vec::with_capacity(n),
        });
        self.by_path.insert(path.to_owned(), id);
        // (in reading order: what an `\input` names is read where it is,
        // before the paragraphs after it, as TeX reads it)
        let mut state = State::default();
        for i in 0..n {
            let pf = self.read_para(id, i, state);
            state = pf.exit;
            self.apply_defs(id, pf.id, &[], &pf.defs());
            let refs = pf.refs();
            self.files[id].paras.push(pf);
            self.follow(id, &refs);
        }
        Some(id)
    }

    fn read_para(&mut self, file: FileId, i: usize, entry: State) -> ParaFacts {
        self.stats.read += 1;
        let p = &self.files[file].tree.paras()[i];
        scan::para_facts(&self.defs, p.id, p.text(), &p.green, entry, self.epoch)
    }

    /// Put `pf` in place of paragraph `i`'s facts: the files it names.
    fn replace_para(&mut self, file: FileId, i: usize, pf: ParaFacts) -> Vec<Ref> {
        let refs = pf.refs();
        let (old_defs, new_defs, id) = (self.files[file].paras[i].defs(), pf.defs(), pf.id);
        self.files[file].paras[i] = pf;
        self.apply_defs(file, id, &old_defs, &new_defs);
        refs
    }

    /// The table of definitions with paragraph `para`'s `old` replaced by
    /// `new`: a change is noted, with every call it reaches.
    fn apply_defs(&mut self, file: FileId, para: ParaId, old: &[Rc<Def>], new: &[Rc<Def>]) {
        let changed = self.defs.replace(file, para, old, new);
        if changed.is_empty() {
            return;
        }
        self.stats.defs_changed += changed.len();
        self.epoch += 1;
        for k in self.defs.dependents(&changed) {
            self.changed_at.insert(k, self.epoch);
        }
        self.dirty = true;
    }

    /// Load the files paragraphs of `file` name.
    fn follow(&mut self, file: FileId, refs: &[Ref]) {
        for r in refs {
            match r {
                Ref::Tex { path, how, sub } => {
                    let base = if *sub {
                        text::dirname(&self.files[file].path).to_owned()
                    } else {
                        String::new()
                    };
                    for c in tex_files(&text::join(&base, path), *how) {
                        if self.load(&c).is_some() {
                            break;
                        }
                    }
                }
                Ref::Bib(path) => {
                    let path = text::normalize(path);
                    if !self.bibs.contains_key(&path) {
                        let bib = self.io.read(&path).map(|t| Bib::scan(&t));
                        self.bibs.insert(path, bib);
                    }
                }
                Ref::Local(path) => {
                    if !self.missing.contains(path)
                        && !self.by_path.contains_key(path)
                        && self.io.exists(path)
                    {
                        self.load(path);
                    } else if !self.by_path.contains_key(path) {
                        self.missing.insert(path.clone());
                    }
                }
            }
        }
    }

    /// Read again every paragraph that calls a definition changed since
    /// it was read, until none does (loading what they name).
    fn settle(&mut self) {
        while self.dirty {
            self.dirty = false;
            let mut f = 0;
            while f < self.files.len() {
                for i in 0..self.files[f].paras.len() {
                    let pf = &self.files[f].paras[i];
                    let stale = pf
                        .calls
                        .iter()
                        .any(|c| self.changed_at.get(c).is_some_and(|&g| g > pf.epoch));
                    if stale {
                        let entry = pf.entry;
                        let new = self.read_para(f, i, entry);
                        let refs = self.replace_para(f, i, new);
                        self.follow(f, &refs);
                    }
                }
                f += 1;
            }
        }
    }

    /// The same document read afresh from its files' texts now (a check
    /// of the incremental state: it must have the same facts).
    #[must_use]
    pub fn fresh(&self) -> Project {
        let texts: HashMap<String, String> = self
            .files
            .iter()
            .map(|f| (f.path.clone(), f.tree.text()))
            .collect();
        let io = Rc::new(Overlay {
            texts,
            under: self.io.clone(),
        });
        Project::open(&self.files[self.main].path, io)
    }

    /// Whether this project's facts are those of [`Project::fresh`]: the
    /// same files, paragraphs, states, facts and calls (`Err`: the first
    /// difference).
    ///
    /// # Errors
    ///
    /// The first difference found.
    pub fn check(&self) -> Result<(), String> {
        let fresh = self.fresh();
        for (id, f) in self.files() {
            let Some(g) = fresh.file_id(&f.path) else {
                // (a file no longer named: kept loaded, but not read)
                continue;
            };
            let a = &f.paras;
            let b = &fresh.files[g].paras;
            if a.len() != b.len() {
                return Err(format!(
                    "{}: {} paragraphs, fresh {}",
                    f.path,
                    a.len(),
                    b.len()
                ));
            }
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                if x.entry != y.entry
                    || x.exit != y.exit
                    || x.facts != y.facts
                    || x.calls != y.calls
                    || x.newlines != y.newlines
                {
                    return Err(format!(
                        "{} (file {id}), paragraph {i}:\n{x:?}\nfresh:\n{y:?}",
                        f.path
                    ));
                }
            }
        }
        for (_, g) in fresh.files() {
            if self.file_id(&g.path).is_none() {
                return Err(format!("{}: loaded afresh, not here", g.path));
            }
        }
        Ok(())
    }
}

/// The files TeX tries for a file read as TeX.
fn tex_files(path: &str, how: InputKind) -> Vec<String> {
    match how {
        InputKind::Include => vec![text::normalize(&format!("{path}.tex"))],
        InputKind::Package => vec![text::normalize(path)],
        _ => text::tex_candidates(path),
    }
}

/// Texts over a host's files.
struct Overlay {
    texts: HashMap<String, String>,
    under: Rc<dyn Files>,
}

impl Files for Overlay {
    fn read(&self, path: &str) -> Option<String> {
        self.texts
            .get(path)
            .cloned()
            .or_else(|| self.under.read(path))
    }

    fn exists(&self, path: &str) -> bool {
        self.texts.contains_key(path) || self.under.exists(path)
    }
}

/// Files in memory (tests, and an editor's unsaved buffers).
#[derive(Default)]
pub struct Memory(pub RefCell<HashMap<String, String>>);

impl Memory {
    #[must_use]
    pub fn new(files: &[(&str, &str)]) -> Rc<Memory> {
        Rc::new(Memory(RefCell::new(
            files
                .iter()
                .map(|(p, t)| ((*p).to_owned(), (*t).to_owned()))
                .collect(),
        )))
    }
}

impl Files for Memory {
    fn read(&self, path: &str) -> Option<String> {
        self.0.borrow().get(path).cloned()
    }

    fn exists(&self, path: &str) -> bool {
        self.0.borrow().contains_key(path)
    }
}
