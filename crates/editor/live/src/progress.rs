//! A build's progress as one record (docs/spec-editor-protocol.md §2):
//! the pages the engine reached in this pass, the packs being fetched.
//! The session's [`Meter`] (its [`crate::session::Status`]'s) makes the
//! records; the viewer's `progress` event ([`crate::view::Viewer::progress`])
//! and the language server's `$/progress` and `phitex/status`
//! ([`crate::lsp::Server::progress`]) are made from them, the same native
//! and wasm.
//!
//! - **Typesetting**: the engine's finishers reaching a new page
//!   ([`phitex_tex::Progress::on_page`]), at most one record per
//!   [`THROTTLE`] (a new pass always told). No timer: a record is made
//!   only when the engine moves.
//! - **Fetching**: the host's files (Shelf's packs, `phitex-shelf`) say
//!   when a pack is asked for and when it is in ([`Meter::fetch`]), with
//!   the packs done and asked so far in this build.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// At most one typesetting record this often.
pub const THROTTLE: Duration = Duration::from_millis(100);

/// What a build is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Fetching,
    /// The format made (not cached: once per PhiTeX update), before the
    /// document.
    Format,
    Typesetting,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::Fetching => "fetching",
            Phase::Format => "format",
            Phase::Typesetting => "typesetting",
        }
    }
}

/// One progress record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    /// This build's engine evaluation, from 1.
    pub pass: u64,
    /// The pages shipped so far in this pass (0: none yet).
    pub pages: u64,
    /// Fetching: the file whose pack is fetched (`amsmath.sty`).
    pub name: Option<String>,
    /// Fetching: the pack's id.
    pub pack: Option<String>,
    /// Fetching: the packs fetched, and asked, so far in this build.
    pub done: Option<u64>,
    pub total: Option<u64>,
}

impl Progress {
    /// The viewer's event: `{"event":"progress", ...}`, the fields that
    /// do not apply left out.
    pub fn event(&self) -> Value {
        let mut v = json!({
            "event": "progress",
            "phase": self.phase.name(),
            "pass": self.pass,
            "pages": self.pages,
        });
        if let Some(o) = v.as_object_mut() {
            if let Some(n) = &self.name {
                o.insert("name".into(), json!(n));
            }
            if let Some(p) = &self.pack {
                o.insert("pack".into(), json!(p));
            }
            if let Some(d) = self.done {
                o.insert("done".into(), json!(d));
            }
            if let Some(t) = self.total {
                o.insert("total".into(), json!(t));
            }
        }
        v
    }
}

/// Where the records go.
type Listener = Box<dyn Fn(&Progress) + Send + Sync>;

#[derive(Default)]
struct State {
    /// The evaluations begun before this build (its first pass is the
    /// next).
    base: u64,
    pass: u64,
    pages: u64,
    /// When the last typesetting record was made.
    told: Option<Instant>,
    done: u64,
    total: u64,
    /// The format is being made ([`Meter::format`]).
    making: bool,
}

/// The session's progress: the records made and given to every
/// listener.
#[derive(Default)]
pub struct Meter {
    state: Mutex<State>,
    listeners: Mutex<Vec<Listener>>,
    /// Told when a package source cannot be reached ([`Meter::unreachable`]).
    unreachable: Mutex<Vec<Box<dyn Fn(&str) + Send + Sync>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Meter {
    /// Every record from now on given to `f` too (on the thread that
    /// makes it: an engine worker's, a fetch's).
    pub fn listen(&self, f: Listener) {
        lock(&self.listeners).push(f);
    }

    fn tell(&self, p: &Progress) {
        for f in lock(&self.listeners).iter() {
            f(p);
        }
    }

    /// `f` told whenever a package source cannot be reached (why).
    pub fn on_unreachable(&self, f: Box<dyn Fn(&str) + Send + Sync>) {
        lock(&self.unreachable).push(f);
    }

    /// A package could not be had: `why` (the host's files: a Shelf pack
    /// unreachable, or refused by the pack table), told to the listeners
    /// (the editor shows it, `phitex_live::lsp::Trouble::Unreachable`).
    pub fn unreachable(&self, why: &str) {
        for f in lock(&self.unreachable).iter() {
            f(why);
        }
    }

    /// A build begins, `evaluations` evaluations begun before it (the
    /// engine's [`phitex_tex::Progress::evaluation`]; 0 for a cold build's
    /// new progress): no pages, no packs yet.
    pub fn build_begins(&self, evaluations: u64) {
        *lock(&self.state) = State {
            base: evaluations,
            ..State::default()
        };
    }

    /// The format is being made (`ready` false: told), or is ready (told
    /// as typesetting from no page, if its making was told).
    pub fn format(&self, ready: bool) {
        let mut s = lock(&self.state);
        let phase = if ready {
            if !s.making {
                return;
            }
            s.making = false;
            Phase::Typesetting
        } else {
            s.making = true;
            Phase::Format
        };
        let p = Progress {
            phase,
            pass: s.pass.max(1),
            pages: 0,
            name: None,
            pack: None,
            done: None,
            total: None,
        };
        drop(s);
        self.tell(&p);
    }

    /// The engine's page hook for `progress` ([`phitex_tex::Progress::on_page`]):
    /// its pages told to this meter.
    pub fn watch(self: &Arc<Self>, progress: &phitex_tex::Progress) {
        let m = self.clone();
        progress.on_page(Box::new(move |evaluation, page| m.page(evaluation, page)));
    }

    /// Evaluation `evaluation` reached page `page`: told, unless a record
    /// of this pass was made less than [`THROTTLE`] ago.
    pub fn page(&self, evaluation: u64, page: u64) {
        let mut s = lock(&self.state);
        let pass = evaluation.saturating_sub(s.base).max(1);
        let now = Instant::now();
        // (within a pass the pages only grow: a page not past the highest
        // seen says nothing; one too soon after the last record is kept
        // for the next)
        if pass == s.pass {
            if page <= s.pages {
                return;
            }
            s.pages = page;
            if s.told.is_some_and(|t| now - t < THROTTLE) {
                return;
            }
        }
        s.pass = pass;
        s.pages = page;
        s.told = Some(now);
        drop(s);
        self.tell(&Progress {
            phase: Phase::Typesetting,
            pass,
            pages: page,
            name: None,
            pack: None,
            done: None,
            total: None,
        });
    }

    /// Pack `pack` is asked for, for file `name` (`arrived`: false), or
    /// is in (true; it could not be had counts as in): told, with the
    /// packs done and asked so far in this build.
    pub fn fetch(&self, name: &str, pack: &str, arrived: bool) {
        let mut s = lock(&self.state);
        if arrived {
            s.done += 1;
        } else {
            s.total += 1;
        }
        let p = Progress {
            phase: Phase::Fetching,
            pass: s.pass.max(1),
            pages: s.pages,
            name: Some(name.to_owned()),
            pack: Some(pack.to_owned()),
            done: Some(s.done.min(s.total)),
            total: Some(s.total),
        };
        drop(s);
        self.tell(&p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(m: &Meter) -> Arc<Mutex<Vec<Progress>>> {
        let got: Arc<Mutex<Vec<Progress>>> = Arc::default();
        let g = got.clone();
        m.listen(Box::new(move |p| lock(&g).push(p.clone())));
        got
    }

    /// Pages are told at most once per 100 ms within a pass; a new pass
    /// at once, its pages counted from it.
    #[test]
    fn pages_throttled_and_passes_told() {
        let m = Meter::default();
        let got = collect(&m);
        m.build_begins(4);
        m.page(5, 1);
        m.page(5, 2);
        m.page(5, 3);
        let g = lock(&got).clone();
        assert_eq!(g.len(), 1);
        assert_eq!((g[0].pass, g[0].pages), (1, 1));
        m.page(6, 1);
        let g = lock(&got).clone();
        assert_eq!(g.len(), 2);
        assert_eq!((g[1].pass, g[1].pages), (2, 1));
        std::thread::sleep(THROTTLE);
        m.page(6, 2);
        assert_eq!(lock(&got).last().map(|p| p.pages), Some(2));
        assert_eq!(
            lock(&got).last().unwrap().event(),
            json!({"event": "progress", "phase": "typesetting", "pass": 2, "pages": 2})
        );
    }

    /// A format made is told as such, and its end as typesetting from no
    /// page; a format read from the cache tells nothing.
    #[test]
    fn a_format_made_is_told_until_ready() {
        let m = Meter::default();
        let got = collect(&m);
        m.build_begins(0);
        m.format(true);
        assert!(lock(&got).is_empty());
        m.format(false);
        m.format(true);
        let g: Vec<Value> = lock(&got).iter().map(Progress::event).collect();
        assert_eq!(
            g,
            [
                json!({"event": "progress", "phase": "format", "pass": 1, "pages": 0}),
                json!({"event": "progress", "phase": "typesetting", "pass": 1, "pages": 0})
            ]
        );
    }

    /// Each pack's ask and arrival told, with done and total so far in
    /// the build; a new build counts from none.
    #[test]
    fn fetches_counted_per_build() {
        let m = Meter::default();
        let got = collect(&m);
        m.build_begins(0);
        m.fetch("amsmath.sty", "amsmath-1", false);
        m.fetch("amsmath.sty", "amsfonts-1", false);
        m.fetch("amsmath.sty", "amsmath-1", true);
        let g = lock(&got).clone();
        assert_eq!(
            g.iter().map(|p| (p.done, p.total)).collect::<Vec<_>>(),
            [(Some(0), Some(1)), (Some(0), Some(2)), (Some(1), Some(2))]
        );
        assert_eq!(
            g[2].event(),
            json!({"event": "progress", "phase": "fetching", "pass": 1, "pages": 0,
                   "name": "amsmath.sty", "pack": "amsmath-1", "done": 1, "total": 2})
        );
        m.build_begins(1);
        m.fetch("x.sty", "x-1", false);
        assert_eq!(lock(&got).last().unwrap().total, Some(1));
    }
}
