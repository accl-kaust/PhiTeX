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

use crate::session::{self, Job, Report, Session, Status};

/// Where past versions come from.
pub trait Versions {
    /// Version `rev`'s files (`rev` as the source names them).
    ///
    /// # Errors
    ///
    /// If there is no such version.
    fn files(&self, rev: &str) -> Result<Box<dyn Files>, String>;
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
    fn files(&self, rev: &str) -> Result<Box<dyn Files>, String> {
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
}

impl Default for Look {
    fn default() -> Look {
        Look {
            markup: "underline".into(),
            subtype: "safe".into(),
            add_color: "#0000ff".into(),
            del_color: "#ff0000".into(),
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
            ..phitex_diff::Options::default()
        }
    }

    fn json(&self) -> Value {
        json!({"markup": self.markup, "subtype": self.subtype, "add_color": self.add_color, "del_color": self.del_color})
    }
}

/// A diff under way: against which version, how it looks, the live diff,
/// and the marked-up document's own session.
///
/// The session is the watch's for its life once made: a later Compare
/// (another pair of versions, the same after a stop) is an edit of the
/// marked-up text it last built, not a clean build. The first is a clean
/// build: the engine starts no session from another's state (its build is
/// one graph of values that cannot be copied, and the diff's job is named
/// otherwise, `JOB-diff`, a value of the job's start that the document's
/// whole build read).
pub struct Compare {
    pub rev: String,
    /// The other side (none: the files as they are, diffed at each edit).
    pub to: Option<String>,
    pub label: String,
    look: Look,
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
    /// The diff's own outputs written (`JOB-diff.*`), for the watcher.
    pub wrote: Vec<(PathBuf, u64)>,
    /// Whether its outputs are written into the folder (the watch's
    /// `--write`): else none (`JOB-diff.pdf`, `.cb`, `.cb2`), its PDF
    /// served from memory.
    write: bool,
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
        job: &Job,
        files: crate::tex::Files,
        status: Arc<Status>,
        write: bool,
    ) -> Result<Compare, String> {
        let pair = Pair::of(src, rev, to, now, main)?;
        let mut j = job.clone();
        j.jobname = format!("{}-diff", job.jobname);
        // (the diff's .aux and .toc its own; the PDF as the document's)
        let session = Session::new(j, files, status, &mut |_| {})?;
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
            wrote: Vec::new(),
            write,
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
        self.live = Live::new(Baseline::from_flat(p.old.clone(), &self.look.options()));
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
        self.live = Live::new(Baseline::from_flat(self.old.clone(), &self.look.options()));
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
            self.session.set_text(crate::tex::chars(tex.as_bytes()));
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
        self.origins = raw.map(|o| self.mapped(&o));
        self.report.as_ref()
    }

    /// Where each change is shown on the diff's pages: the glyphs of the
    /// marked-up text (the main file's), by page, each at its bytes there.
    fn places_of(&self, o: &phitex_live::view::Origins) -> Vec<Option<phitex_diff::Place>> {
        let main = self.session.job.path.file_name();
        let mains: Vec<bool> = o
            .files
            .iter()
            .map(|f| Path::new(f).file_name() == main)
            .collect();
        let glyphs = o.pages.iter().enumerate().flat_map(|(k, p)| {
            let p = p.clone();
            let mains = &mains;
            (0..p.as_ref().map_or(0, |p| p.len())).filter_map(move |i| {
                let p = p.as_ref()?;
                mains.get(p.file[i] as usize).copied().filter(|&m| m)?;
                Some((k, p.y[i], p.start[i] as usize, p.end[i] as usize))
            })
        });
        phitex_diff::places(&self.live.out().changes, glyphs)
    }

    /// The diff build's origins `o` with each glyph of the marked-up text
    /// at its place in the user's files (`DiffOut::source`: a run copied as
    /// it is exactly, a change's markup at the change's place); the glyphs
    /// of other files (a class, a package) kept; the head's, from no file.
    fn mapped(&self, o: &phitex_live::view::Origins) -> phitex_live::view::Origins {
        let d = self.live.out();
        let main = self.session.job.path.file_name();
        let is_main = |f: &str| Path::new(f).file_name() == main;
        let mut files: Vec<String> = d.files.clone();
        let mut index: Vec<u32> = Vec::with_capacity(o.files.len());
        for f in &o.files {
            if is_main(f) {
                index.push(u32::MAX);
            } else {
                index.push(u32::try_from(files.len()).unwrap_or(u32::MAX));
                files.push(f.clone());
            }
        }
        let place = |out: u32| -> Option<(u32, u32)> {
            let (f, at) = d.source(out as usize)?;
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
                            Some(&u32::MAX) => match place(q.start[i]) {
                                Some((k, a)) => {
                                    let b = place(q.end[i].saturating_sub(1))
                                        .filter(|&(k2, b)| k2 == k && b >= a)
                                        .map_or(a + 1, |(_, b)| b + 1);
                                    (q.file[i], q.start[i], q.end[i]) = (k, a, b);
                                }
                                None => q.file[i] = u32::MAX,
                            },
                            Some(&k) => q.file[i] = k,
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

    /// The viewer's diff state: `{shown, base, to, label, changes, look}`.
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
        })
    }

    /// The look the viewer asked for, from its fields (the rest kept).
    pub fn look_from(
        &self,
        markup: Option<&str>,
        subtype: Option<&str>,
        add: Option<&str>,
        del: Option<&str>,
    ) -> Look {
        let l = &self.look;
        Look {
            markup: markup.unwrap_or(&l.markup).to_owned(),
            subtype: subtype.unwrap_or(&l.subtype).to_owned(),
            add_color: add.unwrap_or(&l.add_color).to_owned(),
            del_color: del.unwrap_or(&l.del_color).to_owned(),
        }
    }
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
        }
        .options();
        assert_eq!(o.markup, Markup::Cfont);
        assert_eq!(o.subtype, Subtype::Color);
        assert_eq!(o.add_color.as_ref().map(Color::as_str), Some("#00AA00"));
        assert_eq!(Look::default().options().markup, Markup::Underline);
    }
}
