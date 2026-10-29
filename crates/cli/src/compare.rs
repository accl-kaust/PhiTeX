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
pub struct Compare {
    pub rev: String,
    /// The other side (none: the files as they are, diffed at each edit).
    pub to: Option<String>,
    pub label: String,
    look: Look,
    old: Flat,
    live: Live,
    session: Session,
    /// The marked-up text last built, and its PDF and pages.
    pub tex: String,
    pub pdf: Option<Arc<[u8]>>,
    pub hashes: Vec<u64>,
    pub report: Option<Report>,
    /// The diff's own outputs written (`JOB-diff.*`), for the watcher.
    pub wrote: Vec<(PathBuf, u64)>,
}

/// The main file's flattened text in `files`.
fn flat(files: &dyn Files, main: &str, which: &str) -> Result<Flat, String> {
    phitex_diff::flatten(files, main).map_err(|_| format!("the {which} version has no {main}"))
}

impl Compare {
    /// Diff version `rev` of `main` against `to` (none: the files as they
    /// are, `now`) and build the marked-up document as job `JOB-diff`.
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
    ) -> Result<Compare, String> {
        // (latexdiff's markup is LaTeX's: a plain document can't take it)
        let new_main = match to {
            Some(t) => src.files(t)?.read(main),
            None => now.read(main),
        }
        .unwrap_or_default();
        if !new_main.contains("\\documentclass") {
            return Err(format!(
                "{main} is not a LaTeX document: the diff's markup (latexdiff's \\DIFadd, \\DIFdel) needs LaTeX"
            ));
        }
        let old_files = src.files(rev)?;
        let old =
            flat(old_files.as_ref(), main, "old").or_else(|_| Ok::<_, String>(Flat::default()))?;
        let look = Look::default();
        let live = Live::new(Baseline::from_flat(old.clone(), &look.options()));
        let mut j = job.clone();
        j.jobname = format!("{}-diff", job.jobname);
        // (the diff's .aux and .toc its own; the PDF as the document's)
        let session = Session::new(j, files, status, &mut |_| {})?;
        let short = |r: &str| r.chars().take(7).collect::<String>();
        let label = format!(
            "{} → {}",
            short(rev),
            to.map_or_else(|| "the files as they are".to_owned(), short)
        );
        let mut c = Compare {
            rev: rev.to_owned(),
            to: to.map(str::to_owned),
            label,
            look,
            old,
            live,
            session,
            tex: String::new(),
            pdf: None,
            hashes: Vec::new(),
            report: None,
            wrote: Vec::new(),
        };
        let new = match to {
            Some(t) => flat(src.files(t)?.as_ref(), main, "new")?,
            None => flat(now, main, "new")?,
        };
        c.diff(&new);
        Ok(c)
    }

    /// Diffed again against `new` (only where it changed) and built.
    pub fn diff(&mut self, new: &Flat) {
        let tex = self.live.update(new).tex.clone();
        self.build(tex);
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

    /// The marked-up text built (as an edit of the last one), its outputs
    /// written.
    fn build(&mut self, tex: String) {
        if tex == self.tex && self.report.is_some() {
            return;
        }
        self.session.set_text(crate::tex::chars(tex.as_bytes()));
        self.tex = tex;
        let r = self.session.build();
        self.session.flush_records();
        let ok = r.out.errors.is_empty();
        if let Ok(w) = self.session.write(&r, ok) {
            self.wrote = w.files;
        }
        if let Some(("pdf", b)) = r.main_output() {
            self.pdf = Some(session::shared(b));
            self.hashes.clone_from(&r.out.page_hashes);
        }
        self.report = Some(r);
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
        let list: Value = serde_json::from_str(&phitex_diff::changes_json(changes, &[]))
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
