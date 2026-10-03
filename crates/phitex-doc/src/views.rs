//! The whole document's views, folded from the paragraphs' facts in
//! reading order: the main file's paragraphs, and an `\input`'s file
//! where the `\input` is (its condition, for `\IfFileExists`, decided by
//! what exists).

use std::collections::{BTreeMap, HashMap};

use phitex_syntax::VERBATIM_ENVIRONMENTS;

use crate::defs::Meaning;
use crate::known::{self, AssetKind, CounterOp, Definer, InputKind, LEVELS, Matter};
use crate::{Confirmed, Def, Fact, FileId, FxMap, FxSet, Name, Project, What, text};

/// Where a fact is: a file and a line (from 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Loc {
    pub file: FileId,
    pub line: u32,
}

/// A heading of the outline.
#[derive(Clone, Debug)]
pub struct Heading<'p> {
    /// `\part` −1, `\chapter` 0, `\section` 1, … `\subparagraph` 5.
    pub level: i8,
    pub star: bool,
    /// Its number as LaTeX gives it (`None`: unnumbered).
    pub number: Option<String>,
    pub title: &'p str,
    pub short: Option<&'p str>,
    pub loc: Loc,
    /// Its bytes in its file: the command (or the call of the document's
    /// macro it came from), and its title's text (`None`: not in the
    /// text, read through a macro).
    pub start: u32,
    pub end: u32,
    pub title_range: Option<(u32, u32)>,
    /// The label right after it (`\section{A}\label{sec:a}`).
    pub label: Option<&'p str>,
    pub guarded: bool,
    pub fact: &'p Fact,
}

/// An entry the document writes to a list (`toc`): from a heading, or
/// from `\addcontentsline`.
#[derive(Clone, Debug)]
pub struct TocLine<'p> {
    pub list: &'p str,
    /// `part`, `chapter`, `section`, …
    pub level: &'p str,
    pub number: Option<String>,
    /// The title as written in the source (the short one if given).
    pub title: &'p str,
    pub loc: Loc,
    pub guarded: bool,
    pub fact: &'p Fact,
}

/// A label, a reference or a citation: its key.
#[derive(Clone, Debug)]
pub struct Keyed<'p> {
    pub key: &'p str,
    pub loc: Loc,
    pub guarded: bool,
    pub fact: &'p Fact,
}

/// A file read as TeX, in the graph.
#[derive(Clone, Debug)]
pub struct FileRef<'p> {
    /// How deep (the main file is 0).
    pub depth: usize,
    /// The file found, or the first one tried.
    pub path: String,
    pub how: InputKind,
    pub found: Option<FileId>,
    /// Where it is named (`None`: the main file).
    pub loc: Option<Loc>,
    /// Left out by `\includeonly`.
    pub excluded: bool,
    /// Named while it is being read (TeX would loop): not read again.
    pub cycle: bool,
    pub guarded: bool,
    pub fact: Option<&'p Fact>,
}

/// A file read as data.
#[derive(Clone, Debug)]
pub struct AssetRef<'p> {
    /// The file found, or as named.
    pub path: String,
    pub found: bool,
    pub how: AssetKind,
    pub loc: Loc,
    pub fact: &'p Fact,
}

/// A bibliography database.
#[derive(Clone, Debug)]
pub struct BibRef<'p> {
    pub path: &'p str,
    /// Its keys, if it was found (on disk, or in a `filecontents`).
    pub keys: Option<usize>,
    pub loc: Loc,
}

/// Environments that do not nest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvProblem<'p> {
    /// `\begin{name}` never ended.
    Unclosed { name: &'p str, loc: Loc },
    /// `\end{name}` with no `\begin{name}` open.
    Unopened { name: &'p str, loc: Loc },
    /// `\end{name}` while `\begin{open}` is the innermost.
    Mismatch {
        name: &'p str,
        loc: Loc,
        open: &'p str,
        open_loc: Loc,
    },
}

/// A definition of the document's, where it is.
#[derive(Clone, Debug)]
pub struct DefRef<'p> {
    pub def: &'p Def,
    pub loc: Loc,
}

/// A guard broken: a control sequence (or `{environment}`) whose meaning
/// is not the one the layer reads it with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Broken {
    pub cs: String,
    pub cause: String,
    pub loc: Option<Loc>,
    /// The facts read through it.
    pub facts: usize,
}

/// The meaning a fact assumes of a control sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Meant {
    /// LaTeX's (the kernel's, its class's or a package's: not defined in
    /// the document's files).
    Latex,
    /// The document's definition there.
    Document(Loc),
}

/// What a build said of an assumption.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuardState {
    /// Nothing the layer read contradicts it; no build has confirmed it.
    Assumed,
    /// A build's records confirmed it.
    Kept,
    Broken(String),
}

/// A build's verdict, as [`Project::confirm`] takes it.
pub type Verdict = Confirmed;

/// A control sequence whose meaning the facts assume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assumption {
    /// Its name, or `{environment}`.
    pub cs: String,
    pub meant: Meant,
    /// The facts read through it.
    pub facts: usize,
    pub state: GuardState,
}

/// The whole document's views.
#[derive(Default)]
pub struct Views<'p> {
    pub outline: Vec<Heading<'p>>,
    /// Each file's readings, in reading order: the file, and how many
    /// headings came before it.
    pub entered: Vec<(FileId, usize)>,
    /// The entries of the lists (`toc` and others), in order.
    pub toc: Vec<TocLine<'p>>,
    pub labels: Vec<Keyed<'p>>,
    pub refs: Vec<Keyed<'p>>,
    pub cites: Vec<Keyed<'p>>,
    /// `\nocite{*}`: every key of the databases is cited.
    pub nocite_all: bool,
    /// The file graph, in reading order (the main file first).
    pub files: Vec<FileRef<'p>>,
    pub assets: Vec<AssetRef<'p>>,
    pub bibs: Vec<BibRef<'p>>,
    pub embedded: Vec<(&'p str, &'p str, Loc)>,
    pub includeonly: Option<Vec<&'p str>>,
    pub envs: Vec<EnvProblem<'p>>,
    pub class: Option<&'p str>,
    pub packages: Vec<(&'p str, Loc)>,
    /// The document's definitions read, in reading order.
    pub defs: Vec<DefRef<'p>>,
    /// Definitions of names the layer cannot know.
    pub dynamic: Vec<Loc>,
    pub broken: Vec<Broken>,
    /// The facts read through each control sequence (or `{env}`).
    pub uses: BTreeMap<&'p str, usize>,
    /// The facts made through each of the document's macros.
    pub expanded: BTreeMap<&'p str, usize>,
    /// The keys of the databases found.
    pub bib_keys: FxSet<&'p str>,
    /// The document's macros (and `{environments}`) defined differently
    /// in several places, with how many: their calls are not expanded.
    pub ambiguous: Vec<(String, usize)>,
    labels_by_key: FxMap<&'p str, Vec<usize>>,
}

impl<'p> Views<'p> {
    /// The references to a key no label defines.
    pub fn unresolved(&self) -> impl Iterator<Item = &Keyed<'p>> {
        self.refs
            .iter()
            .filter(|r| !self.labels_by_key.contains_key(r.key))
    }

    /// The keys defined more than once, with their labels.
    #[must_use]
    pub fn duplicates(&self) -> Vec<(&'p str, Vec<&Keyed<'p>>)> {
        let mut out: Vec<(&str, Vec<&Keyed>)> = self
            .labels_by_key
            .iter()
            .filter(|(_, v)| v.len() > 1)
            .map(|(k, v)| (*k, v.iter().map(|&i| &self.labels[i]).collect()))
            .collect();
        out.sort_by_key(|(_, v)| v[0].loc);
        out
    }

    /// The citations of keys no database found has (none if no database
    /// was found: nothing can be said).
    pub fn missing_cites(&self) -> impl Iterator<Item = &Keyed<'p>> {
        let known = self.bibs.iter().any(|b| b.keys.is_some());
        self.cites
            .iter()
            .filter(move |c| known && c.key != "*" && !self.bib_keys.contains(c.key))
    }

    /// Whether a key has a label.
    #[must_use]
    pub fn has_label(&self, key: &str) -> bool {
        self.labels_by_key.contains_key(key)
    }
}

/// The numbers LaTeX's standard classes give headings: the sectioning
/// counters, `secnumdepth`, the matter, the appendix.
struct Numbering {
    /// The class has chapters (book, report, …).
    chapters: bool,
    /// `part` … `subparagraph`.
    c: [i64; 7],
    secnumdepth: i64,
    /// `\mainmatter` (a book's chapters are numbered only there).
    main: bool,
    appendix: bool,
}

impl Numbering {
    fn new() -> Self {
        Numbering {
            chapters: false,
            c: [0; 7],
            secnumdepth: 3,
            main: true,
            appendix: false,
        }
    }

    /// The class: whether it has chapters, and its `secnumdepth`.
    fn class(&mut self, name: &str) {
        if matches!(
            name,
            "book"
                | "report"
                | "memoir"
                | "scrbook"
                | "scrreprt"
                | "amsbook"
                | "extbook"
                | "extreport"
                | "tufte-book"
                | "jsbook"
                | "ltjsbook"
        ) {
            self.chapters = true;
            self.secnumdepth = if name == "memoir" { 1 } else { 2 };
        }
    }

    fn index(name: &str) -> Option<usize> {
        LEVELS.iter().position(|l| *l == name)
    }

    /// One more of counter `i`, the ones below it back to 0.
    fn step(&mut self, i: usize) {
        self.c[i] += 1;
        // (`\part` resets nothing; `\chapter` resets `section` only
        // where there are chapters)
        let from = if i == 0 { 7 } else { i + 1 };
        for j in from..7 {
            self.c[j] = 0;
        }
    }

    /// `\thechapter`, `\thesection`, … for counter `i`.
    fn the(&self, i: usize) -> String {
        let n = self.c[i];
        match i {
            0 => text::roman(n),
            1 => {
                if self.appendix {
                    text::alph(n)
                } else {
                    n.to_string()
                }
            }
            2 if !self.chapters => {
                if self.appendix {
                    text::alph(n)
                } else {
                    n.to_string()
                }
            }
            _ => format!("{}.{n}", self.the(i - 1)),
        }
    }

    /// A heading at `level`: its number, if it gets one.
    fn heading(&mut self, level: i8, star: bool) -> Option<String> {
        if star {
            return None;
        }
        let i = usize::try_from(level + 1).ok()?;
        let numbered = match level {
            -1 => self.secnumdepth > -2,
            0 => self.chapters && self.secnumdepth >= 0 && self.main,
            _ => i64::from(level) <= self.secnumdepth,
        };
        numbered.then(|| {
            self.step(i);
            self.the(i)
        })
    }

    fn counter(&mut self, name: &str, op: CounterOp, value: Option<i64>) {
        if name == "secnumdepth" {
            match (op, value) {
                (CounterOp::Set, Some(v)) => self.secnumdepth = v,
                (CounterOp::Add, Some(v)) => self.secnumdepth += v,
                (CounterOp::Step, _) => self.secnumdepth += 1,
                _ => {}
            }
            return;
        }
        let Some(i) = Self::index(name) else { return };
        match (op, value) {
            (CounterOp::Set, Some(v)) => self.c[i] = v,
            (CounterOp::Add, Some(v)) => self.c[i] += v,
            (CounterOp::Step, _) => self.step(i),
            _ => {}
        }
    }

    fn matter(&mut self, m: Matter) {
        match m {
            Matter::Front | Matter::Back => self.main = false,
            Matter::Main => self.main = true,
            Matter::Appendix => {
                self.appendix = true;
                if self.chapters {
                    self.c[1] = 0;
                    self.c[2] = 0;
                } else {
                    self.c[2] = 0;
                    self.c[3] = 0;
                }
            }
        }
    }
}

/// Where a fact's paragraph is.
#[derive(Clone, Copy)]
struct Ctx<'p> {
    file: FileId,
    para: usize,
    text: &'p str,
    /// The paragraph's offset in its file.
    off: u32,
    /// The paragraph's first line.
    line: u32,
    depth: usize,
}

struct Fold<'p> {
    p: &'p Project,
    v: Views<'p>,
    num: Numbering,
    /// The files being read (an `\input` of one of them is a cycle).
    reading: Vec<FileId>,
    open: Vec<(&'p str, Loc)>,
    /// `\end{document}` read: nothing after it is.
    stopped: bool,
    graphics_path: Vec<&'p str>,
    /// The last heading, while a label right after it may be its: its
    /// index, its paragraph, and where its title ends.
    attach: Option<(usize, FileId, usize, u32)>,
    /// The last line counted: its file, paragraph, offset and line.
    counted: (FileId, usize, usize, u32),
}

impl Project {
    /// The whole document's views.
    #[must_use]
    pub fn views(&self) -> Views<'_> {
        let mut f = Fold {
            p: self,
            v: Views::default(),
            num: Numbering::new(),
            reading: Vec::new(),
            open: Vec::new(),
            stopped: false,
            graphics_path: Vec::new(),
            attach: None,
            counted: (usize::MAX, 0, 0, 0),
        };
        f.v.files.push(FileRef {
            depth: 0,
            path: self.files[self.main].path.clone(),
            how: InputKind::Input,
            found: Some(self.main),
            loc: None,
            excluded: false,
            cycle: false,
            guarded: true,
            fact: None,
        });
        f.file(self.main, 0);
        f.finish()
    }

    /// What the facts assume: each control sequence (or `{environment}`)
    /// read through, with the meaning it must have, and what the
    /// document or a build says of it.
    #[must_use]
    pub fn assumptions(&self) -> Vec<Assumption> {
        let v = self.views();
        let broken: HashMap<&str, &Broken> = v.broken.iter().map(|b| (b.cs.as_str(), b)).collect();
        let state = |cs: &str| match (broken.get(cs), self.verdict(cs)) {
            (Some(b), _) => GuardState::Broken(b.cause.clone()),
            (None, Some(Confirmed::Kept)) => GuardState::Kept,
            (None, Some(Confirmed::Broken(c))) => GuardState::Broken(c.clone()),
            (None, None) => GuardState::Assumed,
        };
        let mut out: Vec<Assumption> = v
            .uses
            .iter()
            .map(|(cs, &facts)| Assumption {
                cs: (*cs).to_owned(),
                meant: Meant::Latex,
                facts,
                state: state(cs),
            })
            .collect();
        for (m, &facts) in &v.expanded {
            let (env, name) = match m.strip_prefix('{') {
                Some(e) => (true, e.trim_end_matches('}')),
                None => (false, *m),
            };
            let Some(loc) = v
                .defs
                .iter()
                .find(|d| d.def.env == env && &*d.def.name == name)
                .map(|d| d.loc)
            else {
                continue;
            };
            out.push(Assumption {
                cs: (*m).to_owned(),
                meant: Meant::Document(loc),
                facts,
                state: state(m),
            });
        }
        out
    }
}

impl<'p> Fold<'p> {
    fn file(&mut self, id: FileId, depth: usize) {
        self.reading.push(id);
        self.v.entered.push((id, self.v.outline.len()));
        let f = &self.p.files[id];
        let mut line = 1;
        let mut off = 0u32;
        for (i, pf) in f.paras.iter().enumerate() {
            if self.stopped {
                break;
            }
            let text = f.tree.paras()[i].text();
            let ctx = Ctx {
                file: id,
                para: i,
                text,
                off,
                line,
                depth,
            };
            for fact in &pf.facts {
                self.fact(fact, ctx);
                if self.stopped {
                    break;
                }
            }
            line += pf.newlines;
            off = off.saturating_add(u32::try_from(text.len()).unwrap_or(u32::MAX));
        }
        self.reading.pop();
    }

    /// A fact's line: counted on from the last one counted, in the
    /// same paragraph and before it, or from the paragraph's start.
    fn loc(&mut self, fact: &Fact, ctx: Ctx<'_>) -> Loc {
        let at = (fact.at as usize).min(ctx.text.len());
        let at = (0..=at)
            .rev()
            .find(|&i| ctx.text.is_char_boundary(i))
            .unwrap_or(0);
        let (file, para, from, line) = self.counted;
        let (from, line) = if file == ctx.file && para == ctx.para && from <= at {
            (from, line)
        } else {
            (0, ctx.line)
        };
        let line = line + text::newlines(&ctx.text[from..at]);
        self.counted = (ctx.file, ctx.para, at, line);
        Loc {
            file: ctx.file,
            line,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn fact(&mut self, fact: &'p Fact, ctx: Ctx<'p>) {
        let loc = self.loc(fact, ctx);
        if !matches!(fact.what, What::If { .. }) {
            *self.v.uses.entry(fact.cs).or_default() += 1;
            if let Some(via) = &fact.via {
                for m in via.iter() {
                    *self.v.expanded.entry(&**m).or_default() += 1;
                }
            }
        }
        let attach = if matches!(fact.what, What::Label(_)) {
            self.attach.take()
        } else {
            self.attach = None;
            None
        };
        match &fact.what {
            What::Heading {
                level,
                star,
                short,
                title,
                end,
                title_at,
            } => {
                let number = self.num.heading(*level, *star);
                if !star {
                    self.v.toc.push(TocLine {
                        list: "toc",
                        level: LEVELS[usize::try_from(level + 1).unwrap_or(0)],
                        number: number.clone(),
                        title: short.as_deref().unwrap_or(title),
                        loc,
                        guarded: true,
                        fact,
                    });
                }
                self.attach = Some((self.v.outline.len(), ctx.file, ctx.para, *end));
                let start = ctx.off.saturating_add(fact.at);
                let title_range = title_at.map(|t| {
                    let t = ctx.off.saturating_add(t);
                    let n = u32::try_from(title.len()).unwrap_or(u32::MAX);
                    (t, t.saturating_add(n))
                });
                self.v.outline.push(Heading {
                    level: *level,
                    star: *star,
                    number,
                    title,
                    short: short.as_deref(),
                    loc,
                    start,
                    end: ctx.off.saturating_add(*end).max(start),
                    title_range,
                    label: None,
                    guarded: true,
                    fact,
                });
            }
            What::Label(key) => {
                if let Some((h, file, para, end)) = attach
                    && file == ctx.file
                    && para == ctx.para
                    && fact.via.is_none()
                    && ctx
                        .text
                        .get(end as usize..fact.at as usize)
                        .is_some_and(|between| text::display(between).is_empty())
                {
                    self.v.outline[h].label = Some(key);
                }
                let i = self.v.labels.len();
                self.v.labels.push(Keyed {
                    key,
                    loc,
                    guarded: true,
                    fact,
                });
                self.v.labels_by_key.entry(key).or_default().push(i);
            }
            What::Ref(keys) => {
                for key in keys {
                    self.v.refs.push(Keyed {
                        key,
                        loc,
                        guarded: true,
                        fact,
                    });
                }
            }
            What::Cite { keys, nocite } => {
                for key in keys {
                    if *nocite && key == "*" {
                        self.v.nocite_all = true;
                    }
                    self.v.cites.push(Keyed {
                        key,
                        loc,
                        guarded: true,
                        fact,
                    });
                }
            }
            What::Input { path, how, sub } => self.input(fact, path, *how, *sub, loc, ctx),
            What::IncludeOnly(list) => {
                self.v.includeonly = Some(list.iter().map(String::as_str).collect());
            }
            What::Bib(files) => {
                for path in files {
                    self.v.bibs.push(BibRef {
                        path,
                        keys: None,
                        loc,
                    });
                }
            }
            What::Asset { path, how } => {
                let candidates: Vec<String> = match how {
                    AssetKind::Graphics => std::iter::once("")
                        .chain(self.graphics_path.iter().copied())
                        .flat_map(|dir| text::graphics_candidates(&format!("{dir}{path}")))
                        .collect(),
                    _ => vec![text::normalize(path)],
                };
                let found = candidates.iter().find(|c| self.p.exists(c));
                self.v.assets.push(AssetRef {
                    path: found.cloned().unwrap_or_else(|| text::normalize(path)),
                    found: found.is_some(),
                    how: *how,
                    loc,
                    fact,
                });
            }
            What::GraphicsPath(dirs) => {
                self.graphics_path = dirs.iter().map(String::as_str).collect();
            }
            What::Embedded { path, text } => self.v.embedded.push((path, text, loc)),
            What::Begin(name) => self.open.push((name, loc)),
            What::End(name) => self.end(name, loc),
            What::Define(def) => {
                if fact.via.is_none() {
                    self.v.defs.push(DefRef { def, loc });
                }
            }
            What::DynamicDefine => self.v.dynamic.push(loc),
            What::Counter { name, op, value } => self.num.counter(name, *op, *value),
            What::Matter(m) => self.num.matter(*m),
            What::Class(name) => {
                if self.v.class.is_none() {
                    self.v.class = Some(name);
                    self.num.class(name);
                }
                self.local(fact, &format!("{name}.cls"), loc, ctx.depth);
            }
            What::Packages(names) => {
                for name in names {
                    self.v.packages.push((name, loc));
                    self.local(fact, &format!("{name}.sty"), loc, ctx.depth);
                }
            }
            What::Toc { list, level, title } => self.v.toc.push(TocLine {
                list,
                level,
                number: None,
                title,
                loc,
                guarded: true,
                fact,
            }),
            What::If { test, then, other } => {
                let exists = text::tex_candidates(test).iter().any(|c| self.p.exists(c));
                for f in if exists { then } else { other } {
                    self.fact(f, ctx);
                    if self.stopped {
                        return;
                    }
                }
            }
        }
    }

    fn input(
        &mut self,
        fact: &'p Fact,
        path: &str,
        how: InputKind,
        sub: bool,
        loc: Loc,
        ctx: Ctx<'p>,
    ) {
        let base = if sub {
            text::dirname(&self.p.files[ctx.file].path)
        } else {
            ""
        };
        let candidates = crate::tex_files(&text::join(base, path), how);
        let found = candidates
            .iter()
            .find_map(|c| self.p.by_path.get(c).copied());
        let excluded = how == InputKind::Include
            && self.v.includeonly.as_ref().is_some_and(|l| {
                !l.iter()
                    .any(|i| text::normalize(i) == text::normalize(path))
            });
        let cycle = found.is_some_and(|f| self.reading.contains(&f));
        self.v.files.push(FileRef {
            depth: ctx.depth + 1,
            path: found.map_or_else(
                || candidates.first().cloned().unwrap_or_default(),
                |f| self.p.files[f].path.clone(),
            ),
            how,
            found,
            loc: Some(loc),
            excluded,
            cycle,
            guarded: true,
            fact: Some(fact),
        });
        if let Some(f) = found
            && !cycle
        {
            self.file(f, ctx.depth + 1);
        }
    }

    /// A class or package of the document's own, read where it is loaded.
    fn local(&mut self, fact: &'p Fact, path: &str, loc: Loc, depth: usize) {
        let Some(&f) = self.p.by_path.get(path) else {
            return;
        };
        let cycle = self.reading.contains(&f);
        self.v.files.push(FileRef {
            depth: depth + 1,
            path: path.to_owned(),
            how: InputKind::Package,
            found: Some(f),
            loc: Some(loc),
            excluded: false,
            cycle,
            guarded: true,
            fact: Some(fact),
        });
        if !cycle {
            self.file(f, depth + 1);
        }
    }

    fn end(&mut self, name: &'p str, loc: Loc) {
        match self.open.iter().rposition(|(n, _)| *n == name) {
            Some(k) if k + 1 == self.open.len() => {
                self.open.pop();
            }
            Some(k) => {
                // (the ones opened inside it were never ended)
                let (open, open_loc) = self.open[self.open.len() - 1];
                self.v.envs.push(EnvProblem::Mismatch {
                    name,
                    loc,
                    open,
                    open_loc,
                });
                for (n, l) in self.open.drain(k + 1..) {
                    if n != open {
                        self.v.envs.push(EnvProblem::Unclosed { name: n, loc: l });
                    }
                }
                self.open.pop();
            }
            None => self.v.envs.push(EnvProblem::Unopened { name, loc }),
        }
        if name == "document" {
            self.stopped = true;
        }
    }

    fn finish(mut self) -> Views<'p> {
        // (those never ended, the innermost first)
        for (name, loc) in std::mem::take(&mut self.open).into_iter().rev() {
            self.v.envs.push(EnvProblem::Unclosed { name, loc });
        }
        // (the databases' keys: on disk, or written by a filecontents)
        let mut keys: FxSet<&'p str> = FxSet::default();
        for b in &mut self.v.bibs {
            let path = text::normalize(b.path);
            let found = self
                .p
                .bib(&path)
                .map(|bib| bib.keys.iter().map(|(k, _)| k.as_str()));
            if let Some(ks) = found {
                let before = keys.len();
                keys.extend(ks);
                b.keys = Some(keys.len() - before);
            } else if let Some((_, text, _)) = self.v.embedded.iter().find(|(p, _, _)| *p == b.path)
            {
                let bib = crate::bib::keys_in(text);
                b.keys = Some(bib.len());
                keys.extend(bib);
            }
        }
        self.v.bib_keys = keys;
        let mut seen = FxSet::default();
        for d in &self.v.defs {
            if seen.insert((d.def.env, &*d.def.name))
                && let Meaning::Ambiguous(n) = self.p.defs.meaning(d.def.env, &d.def.name)
            {
                let name = if d.def.env {
                    format!("{{{}}}", d.def.name)
                } else {
                    d.def.name.to_string()
                };
                self.v.ambiguous.push((name, n));
            }
        }
        self.guards();
        self.v
    }

    /// The guards the document breaks, and the facts read through them.
    fn guards(&mut self) {
        let mut broken: BTreeMap<String, (String, Option<Loc>)> = BTreeMap::new();
        let mut breaks = |cs: &str, cause: String, loc: Option<Loc>| {
            broken.entry(cs.to_owned()).or_insert((cause, loc));
        };
        for d in &self.v.defs {
            let (def, loc) = (d.def, d.loc);
            let by = format!("\\{}", def.cs);
            if def.env {
                if def.definer == Definer::VerbatimEnvironment {
                    breaks(
                        &format!("{{{}}}", def.name),
                        format!("made verbatim by {by}: the syntax tree reads its body as TeX"),
                        Some(loc),
                    );
                } else if VERBATIM_ENVIRONMENTS.contains(&&*def.name) || &*def.name == "document" {
                    breaks(
                        &format!("{{{}}}", def.name),
                        format!("redefined by {by}"),
                        Some(loc),
                    );
                }
                continue;
            }
            if known::construct(&def.name).is_some()
                || phitex_syntax::VERBATIM_COMMANDS.contains(&&*def.name)
            {
                breaks(&def.name, format!("redefined by {by}"), Some(loc));
            }
            if let Some(cs) = known::internal(&def.name) {
                breaks(cs, format!("\\{} redefined by {by}", def.name), Some(loc));
            }
        }
        for &(pkg, loc) in &self.v.packages {
            for cs in known::package_breaks(pkg) {
                breaks(
                    cs,
                    format!("package {pkg}: {}", known::package_cause(pkg)),
                    Some(loc),
                );
            }
        }
        for (cs, verdict) in &self.p.verdicts {
            if let Confirmed::Broken(cause) = verdict {
                breaks(cs, format!("a build: {cause}"), None);
            }
        }
        let bad = |fact: &Fact| {
            broken.contains_key(fact.cs)
                || fact
                    .via
                    .iter()
                    .flat_map(|v| v.iter())
                    .any(|m: &Name| broken.contains_key(&**m))
        };
        for h in &mut self.v.outline {
            h.guarded = !bad(h.fact);
        }
        for t in &mut self.v.toc {
            t.guarded = !bad(t.fact);
        }
        for k in self
            .v
            .labels
            .iter_mut()
            .chain(&mut self.v.refs)
            .chain(&mut self.v.cites)
        {
            k.guarded = !bad(k.fact);
        }
        for f in &mut self.v.files {
            f.guarded = f.fact.is_none_or(|x| !bad(x));
        }
        self.v.broken = broken
            .into_iter()
            .map(|(cs, (cause, loc))| Broken {
                facts: self.v.uses.get(cs.as_str()).copied().unwrap_or(0)
                    + self.v.expanded.get(cs.as_str()).copied().unwrap_or(0),
                cs,
                cause,
                loc,
            })
            .collect();
    }
}
