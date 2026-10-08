//! `phitex watch`'s diff (DESIGN 4.10): the document against a past
//! version from its git repository, latexdiff's markup typeset as a second
//! job beside the watch's.
//!
//! - *The baseline*: `--diff REV`, or the one chosen with `b` (a list of
//!   the repository's commits), else `HEAD`; [`phitex_git`] reads its
//!   files, a [`phitex_diff::Baseline`] keeps it flattened and read.
//! - *The marked-up text*: the working tree is flattened again when one of
//!   the files it read changes, and [`phitex_diff::Live::update`] diffs
//!   again only the paragraphs that changed; it is written to
//!   `.phitex-diff/<job>-diff.tex` when it differs from what is there.
//! - *The job*: a second machine-mode [`Watch`] typesets it, with
//!   `-output-directory=.phitex-diff` (TeX finds figures and `.bib` files
//!   from the project's directory, as it does the document's own). Only
//!   the job shown rebuilds: `d` switches, and the other catches up when
//!   it is shown again; the pages the hidden one ships never reach the
//!   viewer.
//! - *The changes*: placed on the pages by the job's `SyncTeX` file
//!   ([`phitex_diff::places`]); `n` and `N` step through them, the viewer
//!   marking each.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use phitex_diff::{Baseline, ChangeKind, Dir, Flat, Live, Place};

use super::{BUSY, Input, Target, Viewer, main_output};
use crate::machinehost::{Outcome, Watch};
use crate::render::{End, Rebuild, Renderer};

/// Where the diff job's files go, in the project's directory.
pub(super) const DIR: &str = ".phitex-diff";

/// How many commits the baseline list reads.
const LOG: usize = 400;
/// How many lines of the list are shown at once.
const ROWS: usize = 10;

/// A baseline and the live diff against it.
struct Base {
    /// The revision it was taken by, and how it is named (`HEAD~2
    /// (a1b2c3d)`).
    rev: String,
    label: String,
    live: Live,
    /// The working tree's files the last diff read, with their
    /// modification times then (empty: not diffed yet).
    stamps: Vec<(PathBuf, Option<SystemTime>)>,
}

/// The baseline list (`b`): each entry's revision and line, and the one
/// chosen.
struct Picker {
    entries: Vec<(String, String)>,
    at: usize,
}

/// A watch's diff mode.
pub(super) struct Diff {
    repo: Option<phitex_git::Repo>,
    opts: phitex_diff::Options,
    /// The main file, as the project names it.
    main: String,
    /// The diff job's name (`paper-diff`).
    job: String,
    base: Option<Base>,
    watch: Option<Watch>,
    between: crate::Between,
    /// The diff job's last build, and when it began.
    out: Option<(Outcome, Instant)>,
    /// The diff is shown (not the document).
    pub(super) shown: bool,
    /// Where each change of the last build is shown.
    places: Vec<Option<Place>>,
    /// The change `n` and `N` went to last.
    cursor: Option<usize>,
    picker: Option<Picker>,
    formats: Option<PathBuf>,
}

/// A file's modification time.
fn stamp(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// How long ago `time` (seconds since the epoch) was, in a few words.
fn ago(time: i64) -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let s = (now - time).max(0);
    let (n, unit) = match s {
        0..60 => return "just now".into(),
        60..3600 => (s / 60, "minute"),
        3600..86_400 => (s / 3600, "hour"),
        86_400..2_592_000 => (s / 86_400, "day"),
        2_592_000..31_536_000 => (s / 2_592_000, "month"),
        _ => (s / 31_536_000, "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

impl Diff {
    /// The diff mode of the watch of `target` (`rev`: `--diff`'s, taken
    /// now; an error if it names no version).
    pub(super) fn new(
        target: &Target,
        rev: Option<&str>,
        formats: Option<PathBuf>,
    ) -> Result<Diff, String> {
        let repo = phitex_git::Repo::discover(Path::new("."));
        let mut d = Diff {
            repo: None,
            opts: phitex_diff::Options::default(),
            main: phitex_diff::flatten::normalize(&target.file),
            job: format!("{}-diff", target.job()),
            base: None,
            watch: None,
            between: crate::Between::default(),
            out: None,
            shown: false,
            places: Vec::new(),
            cursor: None,
            picker: None,
            formats,
        };
        match repo {
            Ok(r) => d.repo = Some(r),
            Err(e) if rev.is_some() => return Err(format!("--diff: {e}")),
            Err(_) => {}
        }
        if let Some(rev) = rev {
            d.set_baseline(rev)
                .map_err(|e| format!("--diff {rev}: {e}"))?;
        }
        Ok(d)
    }

    /// The marked-up text's file (from the project's directory).
    fn tex(&self) -> String {
        format!("{DIR}/{}.tex", self.job)
    }

    /// Diff against `rev` from now on.
    fn set_baseline(&mut self, rev: &str) -> Result<(), String> {
        let repo = self
            .repo
            .as_ref()
            .ok_or("not in a git repository: there is no past version to diff against")?;
        let snap = repo.snapshot(rev).map_err(|e| e.to_string())?;
        // (a main file the old version has not: everything added)
        let base = Baseline::new(&snap, &self.main, &self.opts)
            .unwrap_or_else(|_| Baseline::from_flat(Flat::default(), &self.opts));
        let label = match snap.commit() {
            Some(id) => {
                let short = id.to_hex_with_len(7).to_string();
                if short.starts_with(rev) || rev.starts_with(&short) {
                    short
                } else {
                    format!("{rev} ({short})")
                }
            }
            None => "the index".into(),
        };
        self.base = Some(Base {
            rev: rev.to_owned(),
            label,
            live: Live::new(base),
            stamps: Vec::new(),
        });
        self.places.clear();
        self.cursor = None;
        Ok(())
    }

    /// The baseline, `HEAD` if none was chosen: false (and why, said) if
    /// there is none.
    fn ensure_baseline(&mut self, ren: &Renderer) -> bool {
        if self.base.is_some() {
            return true;
        }
        match self.set_baseline("HEAD") {
            Ok(()) => true,
            Err(e) => {
                ren.warn("Diff", &e);
                false
            }
        }
    }

    /// Diff again if a file the last diff read changed (or none was made
    /// yet), and write the marked-up text if it differs from what is
    /// there: whether it was written.
    fn refresh(&mut self, ren: &Renderer) -> bool {
        let tex = self.tex();
        let Some(base) = &mut self.base else {
            return false;
        };
        if !base.stamps.is_empty() && base.stamps.iter().all(|(p, t)| stamp(p) == *t) {
            return false;
        }
        let flat = match phitex_diff::flatten(&Dir(PathBuf::from(".")), &self.main) {
            Ok(f) => f,
            Err(e) => {
                ren.warn("Diff", &e);
                return false;
            }
        };
        base.stamps = flat
            .files
            .iter()
            .map(|f| (PathBuf::from(f), stamp(Path::new(f))))
            .collect();
        let out = base.live.update(&flat);
        if std::fs::read(&tex).is_ok_and(|t| t == out.tex.as_bytes()) {
            return false;
        }
        let _ = std::fs::create_dir_all(DIR);
        if let Err(e) = std::fs::write(&tex, &out.tex) {
            ren.warn("Diff", &format!("can't write {tex}: {e}"));
            return false;
        }
        true
    }

    /// Whether the diff is out of date: a file it read changed, or one
    /// its job read.
    fn changed(&mut self) -> bool {
        let edited = self
            .base
            .as_ref()
            .is_some_and(|b| b.stamps.iter().any(|(p, t)| stamp(p) != *t));
        edited || self.watch.as_mut().is_some_and(Watch::changed)
    }

    /// Bring the diff up to date: diffed again and its job rebuilt if
    /// anything changed (built the first time). Whether it built.
    fn update(&mut self, target: &Target, ren: &Renderer, viewer: &Viewer) -> bool {
        if !self.ensure_baseline(ren) {
            return false;
        }
        let written = self.refresh(ren);
        if self.watch.is_some() && !written && !self.watch.as_mut().is_some_and(Watch::changed) {
            return false;
        }
        let quiet = !self.shown;
        if let Some(v) = &viewer.live {
            v.mute(quiet);
        }
        BUSY.store(true, std::sync::atomic::Ordering::Relaxed);
        let t = Instant::now();
        if self.shown {
            viewer.building();
        }
        let mut observe = |p: crate::events::Progress| {
            ren.progress(&p);
            if !quiet {
                viewer.progress(&p);
            }
        };
        let first = self.watch.is_none();
        let out = if let Some(w) = &mut self.watch {
            ren.start_rebuild();
            w.rebuild(&mut self.between, &mut observe)
        } else {
            let label = self.base.as_ref().map_or("", |b| &b.label);
            ren.status(
                "Diffing",
                &format!("{} against {label} ({})", target.file, self.tex()),
            );
            ren.start();
            // (the diff job's command line, set up as the watch's is; then
            // the watch's again, which records and estimates are kept by)
            crate::set_args(self.engine_args(target));
            let mut job = crate::setup();
            crate::set_args(target.engine_args("nonstopmode"));
            job.host.formats.clone_from(&self.formats);
            job.host.notes = true;
            crate::make_output_dir(&job.host);
            let (w, out) = Watch::new(
                job.host,
                job.params,
                job.command_line.as_bytes(),
                &mut self.between,
                &mut observe,
            );
            self.watch = Some(w);
            Some(out)
        };
        BUSY.store(false, std::sync::atomic::Ordering::Relaxed);
        if let Some(v) = &viewer.live {
            v.mute(false);
        }
        let Some(out) = out else {
            ren.idle();
            return false;
        };
        if let Some(w) = &mut self.watch {
            w.join_synctex();
        }
        self.finish(target, ren, &out, !first);
        self.out = Some((out, t));
        self.place();
        if let Some(w) = &mut self.watch {
            w.idle();
        }
        true
    }

    /// The diff job's engine command line: the watch's, on the marked-up
    /// text, its outputs in [`DIR`], with `SyncTeX` (its changes are
    /// placed by it).
    fn engine_args(&self, target: &Target) -> Vec<String> {
        let mut a: Vec<String> = target
            .engine_args_with("nonstopmode", true)
            .into_iter()
            .filter(|a| !a.starts_with("-output-directory="))
            .collect();
        a.pop();
        a.push(format!("-output-directory={DIR}"));
        a.push(self.tex());
        a
    }

    /// Render the end of a diff build.
    fn finish(&self, target: &Target, ren: &Renderer, out: &Outcome, again: bool) {
        let summary = crate::warnings::summarize(&out.diagnostics);
        let output = main_output(&out.outputs);
        let tex = self.tex();
        ren.finish(&End {
            file: &tex,
            summary: &summary,
            term: &out.term,
            bytes: super::output_bytes(&out.outputs, output.as_deref()),
            output,
            failed: out.history > 1,
            checked: false,
            rebuild: again.then(|| Rebuild {
                changed: vec![target.file.clone()],
            }),
        });
        let n = self.changes();
        ren.live().mode(self.shown.then(|| self.mode(n)));
    }

    /// The status line's words for the diff shown.
    fn mode(&self, n: usize) -> String {
        let label = self.base.as_ref().map_or("", |b| &b.label);
        format!(
            "diff against {label}: {}",
            crate::render::plural(n, "change", "changes")
        )
    }

    /// How many changes the last diff found.
    fn changes(&self) -> usize {
        self.base.as_ref().map_or(0, |b| b.live.out().changes.len())
    }

    /// Place the changes on the pages of the last build, by its `SyncTeX`
    /// file: each change where the first record of a line of its markup
    /// is, by page and then down the page.
    fn place(&mut self) {
        self.places.clear();
        let Some(base) = &self.base else { return };
        let out = base.live.out();
        let sync = std::fs::read(format!("{DIR}/{}.synctex.gz", self.job))
            .ok()
            .and_then(|b| crate::synctexfile::text(&b))
            .map(|t| crate::synctexfile::parse(&t))
            .unwrap_or_default();
        let name = format!("{}.tex", self.job);
        let tags: Vec<i32> = sync
            .inputs
            .iter()
            .filter(|(_, n)| n.ends_with(name.as_bytes()))
            .map(|(&t, _)| t)
            .collect();
        let mut starts = vec![0];
        starts.extend(
            out.tex
                .bytes()
                .enumerate()
                .filter(|&(_, c)| c == b'\n')
                .map(|(i, _)| i + 1),
        );
        let mut glyphs = Vec::new();
        for (k, page) in sync.pages.iter().enumerate() {
            for r in page.iter().filter(|r| tags.contains(&r.tag)) {
                let l = usize::try_from(r.line).unwrap_or(0);
                let (Some(&lo), Some(&hi)) = (starts.get(l.wrapping_sub(1)), starts.get(l)) else {
                    continue;
                };
                #[allow(clippy::cast_possible_truncation, reason = "points")]
                glyphs.push((k, r.y as f32, lo, hi));
            }
        }
        glyphs.sort_by(|a, b| {
            (a.0, a.1)
                .partial_cmp(&(b.0, b.1))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        self.places = phitex_diff::places(&out.changes, glyphs);
    }

    /// Show the diff (building it if it is out of date) or the document
    /// (`main`: its last build, begun at `t`).
    fn show(
        &mut self,
        on: bool,
        target: &Target,
        ren: &Renderer,
        viewer: &Viewer,
        main: (&Outcome, Instant),
    ) {
        if on {
            if !self.ensure_baseline(ren) {
                return;
            }
            self.shown = true;
            self.update(target, ren, viewer);
            if let Some((out, t)) = &self.out {
                viewer.built(&out.outputs, out.history, *t, &out.diagnostics);
                let n = self.changes();
                ren.live().mode(Some(self.mode(n)));
                ren.watching(&target.file, main_output(&out.outputs).as_deref());
            }
        } else {
            self.shown = false;
            let (out, t) = main;
            viewer.built(&out.outputs, out.history, t, &out.diagnostics);
            ren.live().mode(None);
            ren.watching(&target.file, main_output(&out.outputs).as_deref());
        }
        if let Some(v) = &viewer.live {
            v.set_diff(&self.state_json());
        }
    }

    /// While the diff is shown: rebuilt if anything changed.
    pub(super) fn poll(&mut self, target: &Target, ren: &Renderer, viewer: &Viewer, asked: bool) {
        if !(asked || self.changed()) {
            return;
        }
        if self.update(target, ren, viewer) {
            if let Some((out, t)) = &self.out {
                viewer.built(&out.outputs, out.history, *t, &out.diagnostics);
                ren.watching(&target.file, main_output(&out.outputs).as_deref());
            }
            if let Some(v) = &viewer.live {
                v.set_diff(&self.state_json());
            }
        } else if asked {
            ren.event("nothing changed");
        }
    }

    /// Go to the next change (`forward`) or the previous one.
    fn step(&mut self, forward: bool, ren: &Renderer, viewer: &Viewer) {
        if !self.shown || self.out.is_none() {
            ren.event("no diff shown (d shows it)");
            return;
        }
        let Some(base) = &self.base else { return };
        let changes = &base.live.out().changes;
        let n = changes.len();
        if n == 0 {
            ren.event(&format!("no changes against {}", base.label));
            return;
        }
        let k = match (self.cursor, forward) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(k), true) => (k + 1) % n,
            (Some(k), false) => (k + n - 1) % n,
        };
        self.cursor = Some(k);
        let c = &changes[k];
        let place = self.places.get(k).copied().flatten();
        let mut line = format!("change {} of {n}", k + 1);
        if let Some(p) = place {
            let _ = write!(line, ", page {}", p.page + 1);
        }
        if let Some(s) = &c.section {
            let _ = write!(line, " · {s}");
        }
        let short = |t: &str| {
            let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
            if t.chars().count() > 48 {
                format!("{}…", t.chars().take(47).collect::<String>())
            } else {
                t
            }
        };
        match c.kind {
            ChangeKind::Add => {
                let _ = write!(line, " · added “{}”", short(&c.new_text));
            }
            ChangeKind::Del => {
                let _ = write!(line, " · deleted “{}”", short(&c.old_text));
            }
            ChangeKind::Change => {
                let _ = write!(
                    line,
                    " · “{}” → “{}”",
                    short(&c.old_text),
                    short(&c.new_text)
                );
            }
        }
        let _ = write!(line, " ({}:{})", c.new.file, line_of(&c.new));
        ren.event(&line);
        if let Some(v) = &viewer.live {
            v.show_bytes(
                &self.tex(),
                c.out.start,
                c.out.end,
                place.map(|p| (p.page, p.y)),
            );
            v.set_diff(&self.state_json());
        }
    }

    /// `D`: the diff written beside the outputs, `<job>-diff.tex` and its
    /// PDF (built first if it is out of date).
    fn write_out(&mut self, target: &Target, ren: &Renderer, viewer: &Viewer) {
        if !self.ensure_baseline(ren) {
            return;
        }
        self.update(target, ren, viewer);
        let Some((out, _)) = &self.out else { return };
        let mut wrote = Vec::new();
        let tex = self.tex();
        let to = target.output(&format!("{}.tex", self.job));
        match std::fs::copy(&tex, &to) {
            Ok(_) => wrote.push(to.display().to_string()),
            Err(e) => ren.warn("Diff", &format!("can't write {}: {e}", to.display())),
        }
        if let Some(pdf) = main_output(&out.outputs) {
            let name = Path::new(&pdf)
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_default();
            let to = target.output(&name.to_string_lossy());
            match std::fs::copy(&pdf, &to) {
                Ok(_) => wrote.push(to.display().to_string()),
                Err(e) => ren.warn("Diff", &format!("can't write {}: {e}", to.display())),
            }
        }
        if !wrote.is_empty() {
            let label = self.base.as_ref().map_or("", |b| &b.label);
            ren.status("Wrote", &format!("{} (against {label})", wrote.join(", ")));
        }
    }

    /// `b`: the list of versions to diff against, shown.
    fn open_picker(&mut self, ren: &Renderer) {
        let Some(repo) = &self.repo else {
            ren.warn(
                "Diff",
                "not in a git repository: there is no past version to diff against",
            );
            return;
        };
        let commits = match repo.log(0, LOG) {
            Ok(c) => c,
            Err(e) => {
                ren.warn("Diff", &e.to_string());
                return;
            }
        };
        let mut entries = vec![(
            phitex_git::STAGED.to_owned(),
            "staged   the index (what `git add` staged)".to_owned(),
        )];
        for c in &commits {
            let mut line = format!("{:<8} ", c.short);
            if !c.refs.is_empty() {
                let names: Vec<&str> = c.refs.iter().map(|r| r.name.as_str()).collect();
                let _ = write!(line, "({}) ", names.join(", "));
            }
            let merge = if c.parents.len() > 1 { "merge: " } else { "" };
            let _ = write!(line, "{merge}{} · {}, {}", c.subject, c.author, ago(c.time));
            entries.push((c.id.to_string(), line));
        }
        // (the baseline now chosen, else HEAD's commit)
        let current = self.base.as_ref().map(|b| b.rev.clone());
        let at = entries
            .iter()
            .position(|(rev, _)| Some(rev) == current.as_ref())
            .unwrap_or(usize::from(entries.len() > 1));
        self.picker = Some(Picker { entries, at });
        self.draw_picker(ren);
    }

    /// The list drawn above the status line.
    fn draw_picker(&self, ren: &Renderer) {
        let Some(p) = &self.picker else {
            ren.live().panel(Vec::new());
            return;
        };
        let s = ren.style();
        let top =
            p.at.saturating_sub(ROWS / 2)
                .min(p.entries.len().saturating_sub(ROWS));
        let mut lines = vec![format!(
            "{} {}",
            s.bold("Diff against:"),
            s.dim("↑/↓ (j/k) choose · Enter diff · Esc cancel")
        )];
        for (i, (_, line)) in p.entries.iter().enumerate().skip(top).take(ROWS) {
            lines.push(if i == p.at {
                format!("{} {}", s.cyan("›"), s.bold(line))
            } else {
                format!("  {line}")
            });
        }
        ren.live().panel(lines);
    }

    /// A key while the list is shown: whether it took it.
    fn picker_key(
        &mut self,
        i: Input,
        target: &Target,
        ren: &Renderer,
        viewer: &Viewer,
        main: (&Outcome, Instant),
    ) -> bool {
        let Some(p) = &mut self.picker else {
            return false;
        };
        match i {
            Input::Up => p.at = p.at.saturating_sub(1),
            Input::Down => p.at = (p.at + 1).min(p.entries.len().saturating_sub(1)),
            Input::Esc | Input::Baseline => {
                self.picker = None;
            }
            Input::Enter => {
                let rev = p.entries[p.at].0.clone();
                self.picker = None;
                self.draw_picker(ren);
                self.choose(&rev, target, ren, viewer, main);
                return true;
            }
            _ => return false,
        }
        self.draw_picker(ren);
        true
    }

    /// Diff against `rev` from now on, and show the diff.
    fn choose(
        &mut self,
        rev: &str,
        target: &Target,
        ren: &Renderer,
        viewer: &Viewer,
        main: (&Outcome, Instant),
    ) {
        match self.set_baseline(rev) {
            Ok(()) => {
                let label = self.base.as_ref().map_or("", |b| &b.label);
                ren.event(&format!("diffing against {label}"));
                self.show(true, target, ren, viewer, main);
            }
            Err(e) => ren.warn("Diff", &e),
        }
    }

    /// A key of the diff's (`d`, `b`, `n`, `N`, `D`, and the list's):
    /// whether it was one. `main` is the document's last build.
    pub(super) fn key(
        &mut self,
        i: Input,
        target: &Target,
        ren: &Renderer,
        viewer: &Viewer,
        main: (&Outcome, Instant),
    ) -> bool {
        if self.picker_key(i, target, ren, viewer, main) {
            return true;
        }
        match i {
            Input::Diff => {
                let on = !self.shown;
                self.show(on, target, ren, viewer, main);
            }
            Input::Baseline => self.open_picker(ren),
            Input::Next => self.step(true, ren, viewer),
            Input::Prev => self.step(false, ren, viewer),
            Input::WriteDiff => self.write_out(target, ren, viewer),
            Input::Viewer => self.asks(target, ren, viewer, main),
            Input::Up | Input::Down | Input::Enter | Input::Esc => {}
            _ => return false,
        }
        true
    }

    /// What the live viewer asked for.
    fn asks(
        &mut self,
        target: &Target,
        ren: &Renderer,
        viewer: &Viewer,
        main: (&Outcome, Instant),
    ) {
        let Some(v) = &viewer.live else { return };
        for ask in v.take_asks() {
            match ask {
                crate::view::Ask::Show(on) => self.show(on, target, ren, viewer, main),
                crate::view::Ask::Step(forward) => self.step(forward, ren, viewer),
                crate::view::Ask::Baseline(rev) => self.choose(&rev, target, ren, viewer, main),
                crate::view::Ask::Write => self.write_out(target, ren, viewer),
            }
        }
    }

    /// The diff's state, for the viewer: `{"shown","base","label",
    /// "changes":[…],"at"}`.
    fn state_json(&self) -> String {
        let mut s = format!("{{\"shown\":{}", self.shown);
        if let Some(b) = &self.base {
            s.push_str(",\"base\":");
            json_str(&mut s, &b.rev);
            s.push_str(",\"label\":");
            json_str(&mut s, &b.label);
            s.push_str(",\"changes\":");
            s.push_str(&phitex_diff::changes_json(
                &b.live.out().changes,
                &self.places,
            ));
        }
        if let Some(k) = self.cursor {
            let _ = write!(s, ",\"at\":{k}");
        }
        s.push_str(",\"file\":");
        json_str(&mut s, &self.tex());
        s.push('}');
        s
    }
}

/// `t` as a JSON string, onto `s`.
fn json_str(s: &mut String, t: &str) {
    let mut b = Vec::new();
    crate::origins::json_str(&mut b, t);
    s.push_str(&String::from_utf8_lossy(&b));
}

/// The line (from 1) a place begins on, in its file as it is now.
fn line_of(l: &phitex_diff::Loc) -> usize {
    std::fs::read(&l.file).map_or(0, |t| {
        t[..l.start.min(t.len())].split(|&c| c == b'\n').count()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages() {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let now = i64::try_from(now).unwrap();
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(now - 120), "2 minutes ago");
        assert_eq!(ago(now - 3600), "1 hour ago");
        assert_eq!(ago(now - 3 * 86_400), "3 days ago");
    }
}
