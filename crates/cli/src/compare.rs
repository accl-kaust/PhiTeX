//! The watch's diff against a past version (the viewer's Compare): the
//! versions come from a [`Versions`] source (here the project's git
//! repository, [`phitex_git`]; Overleaf's extension gives its own history
//! to the same [`phitex_diff`]), the marked-up document (latexdiff's
//! markup) is built by a session of its own beside the document's, and,
//! against the files as they are, diffed again at each edit (the live
//! diff: only what the edit touched) and built again as an edit.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use phitex_diff::{Baseline, Color, Dir, Files, Flat, Live, Markup, Subtype};
use serde_json::{Value, json};

use crate::session::{self, Report, Session, Status};

/// Where past versions come from.
pub trait Versions {
    /// Version `rev`'s files (`rev` as the source names them).
    ///
    /// # Errors
    ///
    /// If there is no such version.
    fn files(&self, rev: &str) -> Result<Box<dyn Files + '_>, String>;
    /// The versions, newest first, `skip` then at most `limit`, as the
    /// viewer's commit graph takes them (`commits.ts`'s `Commit`).
    ///
    /// # Errors
    ///
    /// If the history cannot be read.
    fn log(&self, skip: usize, limit: usize) -> Result<Vec<Value>, String>;
}

/// The project's git repository as a source of versions.
pub struct Git(phitex_git::Repo);

impl Git {
    /// The repository `dir` is in.
    ///
    /// # Errors
    ///
    /// Outside a repository.
    pub fn open(dir: &Path) -> Result<Git, String> {
        phitex_git::Repo::discover(dir)
            .map(Git)
            .map_err(|e| format!("no git repository here ({e})"))
    }
}

impl Versions for Git {
    fn files(&self, rev: &str) -> Result<Box<dyn Files + '_>, String> {
        let s = self.0.snapshot(rev).map_err(|e| format!("{rev}: {e}"))?;
        Ok(Box::new(s))
    }

    fn log(&self, skip: usize, limit: usize) -> Result<Vec<Value>, String> {
        let kind = |k: phitex_git::RefKind| match k {
            phitex_git::RefKind::Head => "head",
            phitex_git::RefKind::Branch => "branch",
            phitex_git::RefKind::Remote => "remote",
            phitex_git::RefKind::Tag => "tag",
        };
        Ok(self
            .0
            .log(skip, limit)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|c| {
                json!({
                    "id": c.id.to_string(),
                    "short": c.short,
                    "parents": c.parents.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    "author": c.author,
                    "time": c.time,
                    "subject": c.subject,
                    "refs": c.refs.iter().map(|r| json!({"name": r.name, "kind": kind(r.kind)})).collect::<Vec<_>>(),
                })
            })
            .collect())
    }
}

/// The files as they are: the editor's buffers over the disk.
pub struct Now<'a> {
    pub root: &'a Path,
    pub buffers: &'a HashMap<String, String>,
}

impl Files for Now<'_> {
    fn read(&self, path: &str) -> Option<String> {
        self.buffers
            .get(path)
            .cloned()
            .or_else(|| Dir(self.root.to_path_buf()).read(path))
    }
}

/// How the diff looks: the viewer's `DiffLook`.
#[derive(Clone, Debug)]
pub struct Look {
    pub markup: String,
    pub subtype: String,
    pub add_color: String,
    pub del_color: String,
    /// The viewer's own styles (Changes' settings): added text, deleted
    /// text, a replaced figure ([`phitex_diff::Style`]); none: the markup's.
    pub ins: Option<String>,
    pub dels: Option<String>,
    pub fig: Option<String>,
}

impl Default for Look {
    fn default() -> Look {
        Look {
            markup: "underline".into(),
            subtype: "safe".into(),
            // (the viewer's default look's: calm, and light on its dark
            // pages)
            add_color: "#1347ae".into(),
            del_color: "#bb1b1b".into(),
            ins: None,
            dels: None,
            fig: None,
        }
    }
}

impl Look {
    fn options(&self) -> phitex_diff::Options {
        phitex_diff::Options {
            markup: Markup::parse(&self.markup).unwrap_or_default(),
            subtype: Subtype::parse(&self.subtype).unwrap_or_default(),
            add_color: Color::parse(&self.add_color).ok(),
            del_color: Color::parse(&self.del_color).ok(),
            style: phitex_diff::Style::parse(
                self.ins.as_deref(),
                self.dels.as_deref(),
                self.fig.as_deref(),
            ),
            ..phitex_diff::Options::default()
        }
    }

    fn json(&self) -> Value {
        json!({"markup": self.markup, "subtype": self.subtype, "add_color": self.add_color, "del_color": self.del_color, "ins_style": self.ins, "del_style": self.dels, "fig_style": self.fig})
    }
}

/// A diff under way: against which version, how it looks, the live diff,
/// and the marked-up document's own session.
///
/// The session starts where the document's stands: a copy of the
/// document's build ([`session::Session::beside`]), the same job (its
/// outputs written as `JOB-diff`), so that its first build is an edit of
/// the document: the marked-up text of each file the diff changed
/// ([`phitex_diff::Split`]: the main file's as its text, the others' as
/// buffers), the markup's definitions, which change nothing an unchanged
/// paragraph reads. It runs again only what the markup touched (the
/// changed paragraphs, the pages from the first one changed until the
/// page breaks are the document's again, what reads them). The session is
/// the watch's for its life once made: a later Compare is an edit of the
/// marked-up text it last built.
pub struct Compare {
    pub rev: String,
    /// The other side (none: the files as they are, diffed at each edit).
    pub to: Option<String>,
    pub label: String,
    pub look: Look,
    old: Flat,
    live: Live,
    session: Session,
    /// Whether the diff is on (the viewer's Compare): off, its session
    /// kept for the next one.
    pub active: bool,
    /// The marked-up text to build next (none: the last one built).
    next: Option<String>,
    /// The marked-up text last built, and its PDF and pages.
    pub tex: String,
    pub pdf: Option<Arc<[u8]>>,
    pub hashes: Vec<u64>,
    pub report: Option<Report>,
    /// The diff's pages' glyph origins, mapped to the user's files through
    /// the diff's map (follow, selection, a double-click on its pages).
    pub origins: Option<phitex_live::view::Origins>,
    /// Where each change is on the diff's pages (its first glyph's page
    /// and height): the viewer's next and previous go there.
    places: Vec<Option<phitex_diff::Place>>,
    /// Each change's box on the page it is first shown on (points from
    /// the page's top left: x0, y0, x1, y1) and that page (from 0).
    boxes: Vec<Option<(usize, [f32; 4])>>,
    /// The diff's own outputs written (`JOB-diff.*`), for the watcher.
    pub wrote: Vec<(PathBuf, u64)>,
    /// Whether its outputs are written into the folder (the watch's
    /// `--write`): else none (`JOB-diff.pdf`, `.cb`, `.cb2`), its PDF
    /// served from memory.
    write: bool,
    /// The marked-up texts given to its session as the files other than
    /// the main one ([`phitex_diff::Split`]), by name: the next build
    /// gives those that changed.
    bufs: HashMap<String, String>,
    /// Whether the text last given was split into the files ([`Compare::give`]).
    split: bool,
    /// How its session began (a copy of the document's build, or why not).
    pub began: String,
}

/// The main file's flattened text in `files`.
fn flat(files: &dyn Files, main: &str, which: &str) -> Result<Flat, String> {
    phitex_diff::flatten(files, main).map_err(|_| format!("the {which} version has no {main}"))
}

/// The two versions a Compare diffs, read.
struct Pair {
    rev: String,
    to: Option<String>,
    old: Flat,
    new: Flat,
}

impl Pair {
    /// Version `rev` of `main` and `to` (none: the files as they are,
    /// `now`), flattened.
    fn of(
        src: &dyn Versions,
        rev: &str,
        to: Option<&str>,
        now: &dyn Files,
        main: &str,
    ) -> Result<Pair, String> {
        let theirs = match to {
            Some(t) => Some(src.files(t)?),
            None => None,
        };
        let new_files: &dyn Files = match &theirs {
            Some(f) => f.as_ref(),
            None => now,
        };
        // (latexdiff's markup is LaTeX's: a plain document can't take it)
        if !new_files
            .read(main)
            .unwrap_or_default()
            .contains("\\documentclass")
        {
            return Err(format!(
                "{main} is not a LaTeX document: the diff's markup (latexdiff's \\DIFadd, \\DIFdel) needs LaTeX"
            ));
        }
        let old_files = src.files(rev)?;
        let old = flat(old_files.as_ref(), main, "old").unwrap_or_default();
        let new = flat(new_files, main, "new")?;
        Ok(Pair {
            rev: rev.to_owned(),
            to: to.map(str::to_owned),
            old,
            new,
        })
    }
}

impl Compare {
    /// Diff version `rev` of `main` against `to` (none: the files as they
    /// are, `now`), the marked-up document to build ([`Compare::build`])
    /// as job `JOB-diff` by a session whose status is `status` (its
    /// progress, its pages streamed: the watch's listeners on it).
    ///
    /// # Errors
    ///
    /// A version not there, or the document's session failing to start.
    #[allow(
        clippy::too_many_arguments,
        reason = "the watch's session's parts, each its own"
    )]
    pub fn start(
        src: &dyn Versions,
        rev: &str,
        to: Option<&str>,
        now: &dyn Files,
        main: &str,
        doc: &Session,
        status: Arc<Status>,
        write: bool,
    ) -> Result<Compare, String> {
        let pair = Pair::of(src, rev, to, now, main)?;
        // (the diff's outputs as `JOB-diff`, its .aux and .toc its own;
        // its job the document's, so that it starts from its build)
        let (session, began) = doc.beside(&format!("{}-diff", doc.job.jobname), status);
        let mut c = Compare {
            rev: String::new(),
            to: None,
            label: String::new(),
            look: Look::default(),
            old: Flat::default(),
            live: Live::new(Baseline::from_flat(
                Flat::default(),
                &Look::default().options(),
            )),
            session,
            active: true,
            next: None,
            tex: String::new(),
            pdf: None,
            hashes: Vec::new(),
            report: None,
            origins: None,
            places: Vec::new(),
            boxes: Vec::new(),
            wrote: Vec::new(),
            write,
            bufs: HashMap::new(),
            split: false,
            began,
        };
        c.take(pair);
        Ok(c)
    }

    /// Diff version `rev` against `to` from now on, in this session, on:
    /// its next build an edit of the marked-up text last built. Nothing
    /// changed on an error.
    ///
    /// # Errors
    ///
    /// A version not there, or not a LaTeX document.
    pub fn against(
        &mut self,
        src: &dyn Versions,
        rev: &str,
        to: Option<&str>,
        now: &dyn Files,
        main: &str,
    ) -> Result<(), String> {
        let pair = Pair::of(src, rev, to, now, main)?;
        self.take(pair);
        self.active = true;
        Ok(())
    }

    fn take(&mut self, p: Pair) {
        let short = |r: &str| r.chars().take(7).collect::<String>();
        self.label = format!(
            "{} → {}",
            short(&p.rev),
            p.to.as_deref()
                .map_or_else(|| "the files as they are".to_owned(), short)
        );
        self.rev = p.rev;
        self.to = p.to;
        self.live = Live::new(Baseline::from_flat(p.old.clone(), &self.options()));
        self.old = p.old;
        self.diff(&p.new);
        // (the last build's PDF, pages and places are of another text: none
        // until this one's build, which may fail before a page)
        if self.next.as_deref() != Some(self.tex.as_str()) {
            self.pdf = None;
            self.hashes.clear();
            self.places.clear();
            self.origins = None;
        }
    }

    /// The diff's options: the look's, changebar's driver the engine's
    /// (XeTeX's for xelatex: pdfTeX's refuses it).
    fn options(&self) -> phitex_diff::Options {
        let mut o = self.look.options();
        if self.session.job.engine.xetex() {
            o.driver = phitex_diff::Driver::Xetex;
        }
        o
    }

    /// The session's status: its phases, its progress and pages.
    pub fn status(&self) -> &Arc<Status> {
        self.session.status()
    }

    /// The marked-up text the next build builds (the last one built if
    /// none is due).
    pub fn text(&self) -> &str {
        self.next.as_deref().unwrap_or(&self.tex)
    }

    /// Diffed again against `new` (only where it changed), to build.
    pub fn diff(&mut self, new: &Flat) {
        self.next = Some(self.live.update(new).tex.clone());
    }

    /// The files as they are diffed again (a live diff only).
    pub fn edited(&mut self, now: &dyn Files, main: &str) {
        if self.to.is_some() {
            return;
        }
        if let Ok(new) = flat(now, main, "new") {
            self.diff(&new);
        }
    }

    /// Files `names` (other than the main one, which the diff flattens)
    /// changed: each the editor's buffer in `buffers`, else as on disk,
    /// read again by the next build, as the document's session is told.
    pub fn files_changed(&mut self, buffers: &HashMap<String, String>, names: &[String]) {
        for n in names {
            // (a file the diff marked up: its marked-up text is the
            // session's, made again from the change by the live diff)
            if self.bufs.contains_key(n) {
                continue;
            }
            let text = buffers.get(n).map(|b| crate::tex::chars(b.as_bytes()));
            self.session.set_buffer(n, text.as_deref());
        }
    }

    /// Every file the builds read looked at again on disk (`r`).
    pub fn reread(&mut self) {
        self.session.reread_all();
    }

    /// A new look: the baseline read again with it, the whole diffed.
    pub fn restyle(&mut self, look: Look, now: &dyn Files, src: &dyn Versions, main: &str) {
        self.look = look;
        self.live = Live::new(Baseline::from_flat(self.old.clone(), &self.options()));
        let new = match &self.to {
            Some(t) => src.files(t).and_then(|f| flat(f.as_ref(), main, "new")),
            None => flat(now, main, "new"),
        };
        if let Ok(new) = new {
            self.diff(&new);
        }
    }

    /// Whether a build is due: a new marked-up text, a file it read
    /// changed, or none built yet.
    pub fn due(&self) -> bool {
        self.report.is_none()
            || self.next.as_ref().is_some_and(|t| *t != self.tex)
            || !self.session.up_to_date()
    }

    /// The marked-up text built, if due (as an edit of the last one), its
    /// outputs written: the build's report, none if none was due.
    pub fn build(&mut self) -> Option<&Report> {
        if let Some(tex) = self.next.take()
            && (tex != self.tex || self.report.is_none())
        {
            if self.report.is_none() {
                self.prime();
            }
            self.give(&tex);
            self.tex = tex;
        }
        if self.report.is_some() && self.session.up_to_date() {
            return None;
        }
        let r = self.session.build();
        self.session.flush_records();
        let ok = r.out.errors.is_empty();
        if self.write
            && let Ok(w) = self.session.write(&r, ok)
        {
            self.wrote = w.files;
        }
        if let Some(("pdf", b)) = r.main_output() {
            self.pdf = Some(session::shared(b));
            self.hashes.clone_from(&r.out.page_hashes);
        }
        self.report = Some(r);
        let raw = self.session.origins();
        self.places = raw.as_ref().map_or_else(Vec::new, |o| self.places_of(o));
        self.boxes = raw.as_ref().map_or_else(Vec::new, |o| self.boxes_of(o));
        self.origins = raw.map(|o| self.mapped(&o));
        self.report.as_ref()
    }

    /// A session started from the document's build: given first the
    /// document with the markup's definitions alone, and built
    /// ([`phitex_diff::with_definitions`]): its paragraphs read none of
    /// them, so that build runs the definitions only, and the next, the
    /// marked-up text, only what the markup changed. (Given at once, the
    /// two make one edit from the body's start to the last change.)
    fn prime(&mut self) {
        let Some(text) = self.session.built_text() else {
            return;
        };
        if let Some(t) = phitex_diff::with_definitions(&text, &self.options()) {
            self.session.set_text(t);
            // (the document's pages again: none streamed to the viewer)
            let _ = self.session.build_unstreamed();
        }
    }

    /// The marked-up text `tex` given to the session: split into the new
    /// version's files where it can be (the main file's as its text, each
    /// other file's that changed since given as its buffer, one given
    /// before and not marked up now as on disk again); else whole, as the
    /// main file's text.
    fn give(&mut self, tex: &str) {
        let out = self.live.out();
        let split = out
            .split
            .as_ref()
            .filter(|s| !s.texts.is_empty() && out.tex == tex);
        self.split = split.is_some();
        let (main, files): (String, HashMap<String, String>) = match split {
            Some(s) => (
                s.texts[0].clone(),
                out.files
                    .iter()
                    .zip(&s.texts)
                    .skip(1)
                    .map(|(n, t)| (n.clone(), t.clone()))
                    .collect(),
            ),
            _ => (tex.to_owned(), HashMap::new()),
        };
        self.session.set_text(crate::tex::chars(main.as_bytes()));
        for (n, t) in &files {
            if self.bufs.get(n) != Some(t) {
                let t = crate::tex::chars(t.as_bytes());
                self.session.set_buffer(n, Some(&t));
            }
        }
        for n in self.bufs.keys() {
            if !files.contains_key(n) {
                self.session.set_buffer(n, None);
            }
        }
        self.bufs = files;
    }

    /// Which of the diff's files ([`phitex_diff::DiffOut::files`]) file
    /// `f` of the build is: the main file by its name, the others by
    /// their paths from the project's folder.
    fn file_of(&self, f: &str) -> Option<usize> {
        let d = self.live.out();
        let main = self.session.job.path.file_name();
        if Path::new(f).file_name() == main {
            return Some(0);
        }
        d.split.as_ref()?;
        let plain = f.strip_prefix("./").unwrap_or(f);
        d.files
            .iter()
            .skip(1)
            .position(|x| plain == x || Path::new(plain).ends_with(x))
            .map(|k| k + 1)
    }

    /// Byte `at` of the build's file `k` of the diff's files, in the
    /// flat marked-up text ([`phitex_diff::DiffOut::tex`]).
    fn flat_at(&self, k: usize, at: usize) -> Option<usize> {
        match &self.live.out().split {
            Some(s) if self.split => s.flat_of(k, at),
            _ => (k == 0).then_some(at),
        }
    }

    /// Where each change is shown on the diff's pages: the glyphs of the
    /// marked-up text (its files'), by page, each at its bytes there.
    fn places_of(&self, o: &phitex_live::view::Origins) -> Vec<Option<phitex_diff::Place>> {
        let ours: Vec<Option<usize>> = o.files.iter().map(|f| self.file_of(f)).collect();
        let glyphs = o.pages.iter().enumerate().flat_map(|(k, p)| {
            let p = p.clone();
            let ours = &ours;
            (0..p.as_ref().map_or(0, |p| p.len())).filter_map(move |i| {
                let p = p.as_ref()?;
                let f = (*ours.get(p.file[i] as usize)?)?;
                let a = self.flat_at(f, p.start[i] as usize)?;
                let b = self
                    .flat_at(f, (p.end[i] as usize).saturating_sub(1))
                    .map_or(a + 1, |b| b + 1);
                Some((k, p.y[i], a, b.max(a + 1)))
            })
        });
        phitex_diff::places(&self.live.out().changes, glyphs)
    }

    /// Each change's box on the page its first glyph is on: the glyphs of
    /// its markup there.
    fn boxes_of(&self, o: &phitex_live::view::Origins) -> Vec<Option<(usize, [f32; 4])>> {
        let changes = &self.live.out().changes;
        let mut out: Vec<Option<(usize, [f32; 4])>> = vec![None; changes.len()];
        let ours: Vec<Option<usize>> = o.files.iter().map(|f| self.file_of(f)).collect();
        for (k, p) in o.pages.iter().enumerate() {
            let Some(p) = p else { continue };
            for i in 0..p.len() {
                let Some(f) = ours.get(p.file[i] as usize).copied().flatten() else {
                    continue;
                };
                let Some(a) = self.flat_at(f, p.start[i] as usize) else {
                    continue;
                };
                let mut c = changes.partition_point(|c| c.out.end <= a);
                while c < changes.len() && changes[c].out.start <= a {
                    let g = glyph_box(p, i);
                    match &mut out[c] {
                        None => out[c] = Some((k, g)),
                        Some((page, b)) if *page == k => *b = union(*b, g),
                        Some(_) => {}
                    }
                    c += 1;
                }
            }
        }
        out
    }

    /// Where what line `line` (from 1) of the user's file `file` made is on
    /// the diff's pages: the page it is first on, and its glyphs' box
    /// there.
    pub fn line_box(&self, file: &str, text: &str, line: usize) -> Option<(usize, [f32; 4])> {
        let o = self.origins.as_ref()?;
        let a = text
            .split_inclusive('\n')
            .take(line - 1)
            .map(str::len)
            .sum::<usize>();
        let b = a + text[a..].find('\n').map_or(text.len() - a, |n| n + 1);
        let plain = file.strip_prefix("./").unwrap_or(file);
        let fs: Vec<bool> = o
            .files
            .iter()
            .map(|f| {
                let f = f.strip_prefix("./").unwrap_or(f);
                f == plain || Path::new(f).ends_with(plain)
            })
            .collect();
        let mut found: Option<(usize, [f32; 4])> = None;
        for (k, p) in o.pages.iter().enumerate() {
            let Some(p) = p else { continue };
            for i in 0..p.len() {
                let at = p.start[i] as usize;
                if fs.get(p.file[i] as usize) == Some(&true) && (a..b).contains(&at) {
                    match &mut found {
                        None => found = Some((k, glyph_box(p, i))),
                        Some((page, bx)) if *page == k => *bx = union(*bx, glyph_box(p, i)),
                        Some(_) => {}
                    }
                }
            }
            if found.is_some() {
                break;
            }
        }
        found
    }

    /// The diff's PDF as last built.
    pub fn pdf(&self) -> Option<&Arc<[u8]>> {
        self.pdf.as_ref()
    }

    /// The changes as `session compare` gives them: each its kind, its
    /// page (from 1) and box (points from the page's top left), its place
    /// in the user's files (a file and a line, from 1), and its texts.
    pub fn changes_for(&self, now: &dyn Files) -> Vec<Value> {
        let d = self.live.out();
        let mut texts: HashMap<String, Option<String>> = HashMap::new();
        d.changes
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let t = texts
                    .entry(c.new.file.clone())
                    .or_insert_with(|| now.read(&c.new.file));
                let line = t
                    .as_ref()
                    .map(|t| t.get(..c.new.start).map_or(0, |s| s.matches('\n').count()) + 1);
                let kind = match c.kind {
                    phitex_diff::ChangeKind::Add => "add",
                    phitex_diff::ChangeKind::Del => "del",
                    phitex_diff::ChangeKind::Change => "change",
                };
                let r = |x: f32| (f64::from(x) * 100.0).round() / 100.0;
                let (page, bbox) = match self.boxes.get(i).copied().flatten() {
                    Some((k, b)) => (json!(k + 1), json!(b.map(r))),
                    None => (Value::Null, Value::Null),
                };
                json!({
                    "n": i + 1,
                    "kind": kind,
                    "page": page,
                    "bbox": bbox,
                    "file": c.new.file,
                    "line": line,
                    "old_text": c.old_text,
                    "new_text": c.new_text,
                    "section": c.section,
                })
            })
            .collect()
    }

    /// The diff build's origins `o` with each glyph of the marked-up text
    /// at its place in the user's files (`DiffOut::source`: a run copied as
    /// it is exactly, a change's markup at the change's place); the glyphs
    /// of other files (a class, a package) kept; the head's, from no file.
    fn mapped(&self, o: &phitex_live::view::Origins) -> phitex_live::view::Origins {
        let d = self.live.out();
        let mut files: Vec<String> = d.files.clone();
        // (each of the build's files: one of the diff's (its glyphs mapped
        // through the diff's map), or another kept as it is)
        let mut index: Vec<Result<usize, u32>> = Vec::with_capacity(o.files.len());
        for f in &o.files {
            if let Some(k) = self.file_of(f) {
                index.push(Ok(k));
            } else {
                index.push(Err(u32::try_from(files.len()).unwrap_or(u32::MAX)));
                files.push(f.clone());
            }
        }
        let place = |k: usize, at: u32| -> Option<(u32, u32)> {
            let out = self.flat_at(k, at as usize)?;
            let (f, at) = d.source(out)?;
            let k = d.files.iter().position(|x| x == f)?;
            Some((u32::try_from(k).ok()?, u32::try_from(at).ok()?))
        };
        let pages = o
            .pages
            .iter()
            .map(|p| {
                p.as_ref().map(|p| {
                    let mut q = (**p).clone();
                    for i in 0..q.file.len() {
                        match index.get(q.file[i] as usize) {
                            Some(&Ok(f)) => match place(f, q.start[i]) {
                                Some((k, a)) => {
                                    let b = place(f, q.end[i].saturating_sub(1))
                                        .filter(|&(k2, b)| k2 == k && b >= a)
                                        .map_or(a + 1, |(_, b)| b + 1);
                                    (q.file[i], q.start[i], q.end[i]) = (k, a, b);
                                }
                                None => q.file[i] = u32::MAX,
                            },
                            Some(&Err(k)) => q.file[i] = k,
                            None => {}
                        }
                    }
                    Arc::new(q)
                })
            })
            .collect();
        phitex_live::view::Origins { files, pages }
    }

    /// The marked-up document's errors, each as a compiler's report.
    pub fn errors(&self, color: bool) -> Vec<String> {
        self.report.as_ref().map_or_else(Vec::new, |r| {
            let placed = crate::problems::rendered(&self.session, r, false, color, false);
            if placed.is_empty() {
                r.out.errors.clone()
            } else {
                placed
            }
        })
    }

    /// The viewer's diff state: `{shown, base, to, label, changes, look,
    /// errors, built}`.
    pub fn state(&self, shown: bool) -> Value {
        let changes = &self.live.out().changes;
        let list: Value = serde_json::from_str(&phitex_diff::changes_json(changes, &self.places))
            .unwrap_or_else(|_| json!([]));
        json!({
            "shown": shown,
            "base": self.rev,
            "to": self.to,
            "label": self.label,
            "changes": list,
            "look": self.look.json(),
            "errors": self.report.as_ref().map_or(0, |r| r.out.errors.len()),
            "built": self.built(),
        })
    }

    /// Whether the marked-up text is the one last built: the changes'
    /// places are of its pages. False while a new text waits for its build
    /// (a Compare's first state, told before it is typeset): a change with
    /// no place yet may have one once it is.
    fn built(&self) -> bool {
        self.report.is_some() && self.next.as_ref().is_none_or(|t| *t == self.tex)
    }

    /// The look the viewer asked for, from its fields (the rest kept).
    pub fn look_from(&self, f: &[Option<String>; 7]) -> Look {
        let l = &self.look;
        let or = |k: usize, d: &str| f[k].as_deref().unwrap_or(d).to_owned();
        Look {
            markup: or(0, &l.markup),
            subtype: or(1, &l.subtype),
            add_color: or(2, &l.add_color),
            del_color: or(3, &l.del_color),
            ins: f[4].clone().or_else(|| l.ins.clone()),
            dels: f[5].clone().or_else(|| l.dels.clone()),
            fig: f[6].clone().or_else(|| l.fig.clone()),
        }
    }
}

/// Glyph `i`'s box on its page: its advance, from its size's ascent
/// above the baseline to its descent below.
fn glyph_box(p: &phitex_tex::PageOrigins, i: usize) -> [f32; 4] {
    let (x, y, w, s) = (p.x[i], p.y[i], p.w[i], p.size[i]);
    [x, y - 0.75 * s, x + w, y + 0.25 * s]
}

fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

/// An older copy of the main file as a version: the files as they are,
/// the main one's text the copy's (`session compare PATH`).
struct OldCopy<'a> {
    main: String,
    text: String,
    now: &'a dyn Files,
}

impl Files for OldCopy<'_> {
    fn read(&self, path: &str) -> Option<String> {
        if path == self.main {
            Some(self.text.clone())
        } else {
            self.now.read(path)
        }
    }
}

/// [`OldCopy`] as the source of the one version, whatever it is named.
struct CopySource<'a> {
    main: String,
    text: String,
    now: &'a dyn Files,
}

impl Versions for CopySource<'_> {
    fn files(&self, _rev: &str) -> Result<Box<dyn Files + '_>, String> {
        Ok(Box::new(OldCopy {
            main: self.main.clone(),
            text: self.text.clone(),
            now: self.now,
        }))
    }

    fn log(&self, _skip: usize, _limit: usize) -> Result<Vec<Value>, String> {
        Ok(Vec::new())
    }
}

/// `phitex session compare` in the watch (and `png --compare`): the agent's
/// own diff (`slot`, beside the viewer's, which it leaves as it is) of the
/// files as they are (`root`, `buffers`) against `rev` (a git revision, or
/// the path of an older copy of the main file `main`; none: the last one),
/// started from the document's session `doc`, in look `look`, built; its
/// PDF written to `.phitex/compare/JOB-diff.pdf`; `at`'s place on its
/// pages. `stop`: the agent's diff ended.
///
/// # Errors
///
/// No version to compare with, a version not there, not a LaTeX document.
#[allow(clippy::too_many_arguments, reason = "the watch's parts, each its own")]
pub fn agent(
    slot: &mut Option<Compare>,
    doc: &Session,
    root: &Path,
    main: &str,
    buffers: &HashMap<String, String>,
    rev: Option<&str>,
    stop: bool,
    look: &[Option<String>; 4],
    at: Option<&(String, usize)>,
) -> Result<Value, String> {
    if stop {
        let was = slot.take().is_some();
        return Ok(json!({"stopped": was}));
    }
    let t0 = std::time::Instant::now();
    let now = Now { root, buffers };
    let rev = match (rev, slot.as_ref()) {
        (Some(r), _) => r.to_owned(),
        (None, Some(c)) => c.rev.clone(),
        (None, None) => return Err("no version to compare with: give a revision".into()),
    };
    // (a file here: an older copy of the main file; else a git revision)
    let copy = [root.join(&rev), PathBuf::from(&rev)]
        .into_iter()
        .find(|p| p.is_file())
        .map(|p| {
            std::fs::read(&p)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .map_err(|e| format!("{}: {e}", p.display()))
        })
        .transpose()?;
    let src: Box<dyn Versions + '_> = match copy {
        Some(text) => Box::new(CopySource {
            main: main.to_owned(),
            text,
            now: &now,
        }),
        None => Box::new(Git::open(root)?),
    };
    match slot.as_mut() {
        Some(c) => c.against(src.as_ref(), &rev, None, &now, main)?,
        None => {
            *slot = Some(Compare::start(
                src.as_ref(),
                &rev,
                None,
                &now,
                main,
                doc,
                Arc::new(Status::default()),
                false,
            )?);
        }
    }
    let c = slot.as_mut().ok_or("no diff")?;
    if look.iter().any(Option::is_some) {
        let l = c.look_from(&[
            look[0].clone(),
            look[1].clone(),
            look[2].clone(),
            look[3].clone(),
            None,
            None,
            None,
        ]);
        c.restyle(l, &now, src.as_ref(), main);
    }
    c.build();
    let pdf = match c.pdf() {
        Some(b) => {
            let dir = root.join(".phitex/compare");
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let p = dir.join(format!("{}-diff.pdf", doc.job.jobname));
            std::fs::write(&p, &b[..]).map_err(|e| format!("{}: {e}", p.display()))?;
            Some(p.display().to_string())
        }
        None => None,
    };
    // (the marked-up files as the build was given them, beside it: what
    // the diff's pages are made of)
    let tex_dir = root.join(".phitex/compare/tex");
    let _ = std::fs::remove_dir_all(&tex_dir);
    {
        let d = c.live.out();
        let files: Vec<(&str, &str)> = match (&d.split, c.split) {
            (Some(sp), true) => d
                .files
                .iter()
                .map(String::as_str)
                .zip(sp.texts.iter().map(String::as_str))
                .collect(),
            _ => vec![(main, c.tex.as_str())],
        };
        for (n, t) in files {
            let p = tex_dir.join(n);
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            std::fs::write(&p, t).map_err(|e| format!("{}: {e}", p.display()))?;
        }
    }
    let changes = c.changes_for(&now);
    let region = at.and_then(|(f, l)| {
        let text = now.read(f)?;
        let (k, b) = c.line_box(f, &text, *l)?;
        let r = |x: f32| (f64::from(x) * 100.0).round() / 100.0;
        Some(json!({"page": k + 1, "bbox": b.map(r)}))
    });
    let r = c.report.as_ref();
    Ok(json!({
        "rev": c.rev,
        "label": c.label,
        "pdf": pdf,
        "tex": tex_dir.display().to_string(),
        "pages": c.hashes.len(),
        "changes": changes,
        "errors": c.errors(false),
        "look": c.look.json(),
        "ms": t0.elapsed().as_secs_f64() * 1e3,
        "build_ms": r.map(|r| r.wall.as_secs_f64() * 1e3),
        "began": c.began,
        "at": region,
        "runs": r.map(|r| r.runs.iter().map(|x| x.commands_run).sum::<u64>()),
        "runs_by_op": c
            .session
            .runs_by_op()
            .into_iter()
            .map(|(k, v)| (k.to_owned(), json!(v)))
            .collect::<serde_json::Map<String, Value>>(),
        // (the nodes its build ran again, with their texts: which
        // paragraphs, `PHITEX_DIFF_RERAN=1`)
        "reran": std::env::var_os("PHITEX_DIFF_RERAN").map(|_| {
            c.session
                .reran_texts()
                .into_iter()
                .map(|(op, t, why)| json!({"op": op, "text": t, "why": why}))
                .collect::<Vec<_>>()
        }),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_over_the_disk() {
        let dir = std::env::temp_dir().join(format!("phitex-compare-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.tex"), "disk").unwrap();
        std::fs::write(dir.join("b.tex"), "disk b").unwrap();
        let buffers = HashMap::from([("a.tex".to_owned(), "buffer".to_owned())]);
        let now = Now {
            root: &dir,
            buffers: &buffers,
        };
        assert_eq!(now.read("a.tex").as_deref(), Some("buffer"));
        assert_eq!(now.read("b.tex").as_deref(), Some("disk b"));
        assert_eq!(now.read("c.tex"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn looks_as_the_viewer_names_them() {
        let o = Look {
            markup: "cfont".into(),
            subtype: "color".into(),
            add_color: "#00aa00".into(),
            del_color: "red".into(),
            ins: None,
            dels: None,
            fig: None,
        }
        .options();
        assert_eq!(o.markup, Markup::Cfont);
        assert_eq!(o.subtype, Subtype::Color);
        assert_eq!(o.add_color.as_ref().map(Color::as_str), Some("#00AA00"));
        assert_eq!(Look::default().options().markup, Markup::Underline);
    }
}
