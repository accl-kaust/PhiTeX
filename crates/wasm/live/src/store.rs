//! The session's store ([`phitex_live::host::Store`]) as the host keeps it
//! (VS Code's global storage, the browser's IndexedDB): bytes by a key the
//! host scopes itself, `doc/NAME` the document's own (`wkeys`, `wnames`,
//! `noderec`), `format/NAME` a format's evaluated state (its 32-byte key
//! first, then the state) and `format/NAME/pkgrec` the package loads'
//! records beside it; NAME is the session's own (the engine, its identity,
//! the ini's hash), so a format another engine made is never asked for.
//!
//! Asked when the session needs it, as the files are ([`crate::files`]):
//! `phitex.store_get` posted to the host from the thread that needs it,
//! which waits on a condition variable until thread 0 answers
//! (`ph_stored`). Lazily rather than all given at the open: the host does
//! not know the format's name (the engine's identity is the core's), and
//! a session asks a handful of keys, once each, the format's state only at
//! a cold build. A put is posted with its bytes (`phitex.store_put`) and
//! not waited for. The document's keys are kept here once asked or put,
//! so the session's compare before a put costs no round trip; a format's
//! state is not (tens of MB, read once).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

use phitex_live::host::{Key, Store};

/// Where an ask (the key) and a put (the key and bytes) are posted.
pub type Ask = fn(&str);
pub type Put = fn(&str, &[u8]);

enum Slot {
    Asked,
    Given(Option<Vec<u8>>),
}

pub struct Kept {
    slots: Mutex<HashMap<String, Slot>>,
    answered: Condvar,
    ask: Ask,
    put: Put,
    /// The core ended ([`Kept::end`]; set with `slots` held).
    ended: AtomicBool,
    /// The persisted graph (`phitex_live::graph`, the store's `doc/graph`),
    /// which the host keeps where it keeps a document's (VS Code's
    /// workspace storage, the browser's `IndexedDB`), not as the session's
    /// other keys: tens of MB, given and taken whole by the host's own
    /// calls, never asked of it or posted ([`Graph`]).
    graph: Mutex<Graph>,
}

/// The persisted graph's bytes between the core and its host.
#[derive(Default)]
struct Graph {
    /// What the host gave before the open (`ph_graph_give`): the session's
    /// first build takes it (none given: none saved, or graphs off).
    given: Option<Vec<u8>>,
    /// The graph given was refused (cut, a hash wrong): the host drops its
    /// copy (told once, [`Kept::graph_dropped`]).
    dropped: bool,
}

/// The persisted graph's key in the session's store.
const GRAPH: &str = "doc/graph";

thread_local! {
    /// The graph the session saved on this thread (its session thread's;
    /// without threads, the host's one), until [`Kept::take_graph`].
    static SAVED: std::cell::RefCell<Option<Vec<u8>>> = const { std::cell::RefCell::new(None) };
}

impl Kept {
    pub fn new(ask: Ask, put: Put) -> Kept {
        Kept {
            slots: Mutex::default(),
            answered: Condvar::new(),
            ask,
            put,
            ended: AtomicBool::new(false),
            graph: Mutex::default(),
        }
    }

    fn graph(&self) -> MutexGuard<'_, Graph> {
        self.graph.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The graph the host saved for the document about to open (none: it
    /// has none, or keeps none): its first build takes it.
    pub fn give_graph(&self, bytes: Option<Vec<u8>>) {
        let mut g = self.graph();
        g.given = bytes;
        g.dropped = false;
    }

    /// The graph the session of this thread saved last, handed over
    /// (none: none since). Each session's own: a document closed while
    /// its save ended never hands its bytes to the next one's.
    pub fn take_graph() -> Option<Vec<u8>> {
        SAVED.with(std::cell::RefCell::take)
    }

    /// Whether the graph given was refused since last asked: the host
    /// drops its copy.
    pub fn graph_dropped(&self) -> bool {
        std::mem::take(&mut self.graph().dropped)
    }

    /// The core ended (`ph_end`): the host hears nothing more, so nothing
    /// is asked or put, a key not had is empty, and every thread waiting
    /// for one goes on. Nothing a build makes after its core's end is kept:
    /// it read files the host no longer gave.
    pub fn end(&self) {
        let s = self.lock();
        self.ended.store(true, Ordering::SeqCst);
        drop(s);
        self.answered.notify_all();
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Slot>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The host's answer for `key` (none: it has nothing under it).
    pub fn stored(&self, key: &str, bytes: Option<&[u8]>) {
        let mut s = self.lock();
        if let Some(slot) = s.get_mut(key) {
            *slot = Slot::Given(bytes.map(<[u8]>::to_vec));
        }
        drop(s);
        self.answered.notify_all();
    }

    /// The bytes under `key`, asked of the host once; kept (`keep`) for the
    /// next ask, else handed over.
    fn get_key(&self, key: &str, keep: bool) -> Option<Vec<u8>> {
        let mut s = self.lock();
        loop {
            match s.get(key) {
                // (the core ended: nothing comes that was not here)
                None | Some(Slot::Asked) if self.ended.load(Ordering::SeqCst) => return None,
                // (not asked yet, or handed over to another asker)
                None => {
                    s.insert(key.to_owned(), Slot::Asked);
                    // (the lock let go: a host without threads answers inside the call)
                    drop(s);
                    (self.ask)(key);
                    s = self.lock();
                }
                Some(Slot::Given(b)) if keep => return b.clone(),
                Some(Slot::Given(_)) => {
                    let Some(Slot::Given(b)) = s.remove(key) else {
                        unreachable!("given above")
                    };
                    return b;
                }
                // (no thread to answer later: not answered in the call, nothing kept)
                _ if !crate::THREADED => {
                    s.remove(key);
                    return None;
                }
                _ => {
                    s = self
                        .answered
                        .wait(s)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            }
        }
    }

    fn put_key(&self, key: &str, bytes: &[u8], keep: bool) {
        let mut s = self.lock();
        if self.ended.load(Ordering::SeqCst) {
            return;
        }
        if keep {
            s.insert(key.to_owned(), Slot::Given(Some(bytes.to_vec())));
        } else if matches!(s.get(key), Some(Slot::Given(_))) {
            s.remove(key);
        }
        drop(s);
        (self.put)(key, bytes);
    }
}

fn name(key: Key<'_>) -> String {
    match key {
        Key::Doc(f) => format!("doc/{f}"),
        Key::Format(n, f) => format!("format/{n}/{f}"),
    }
}

impl Store for Kept {
    fn get(&self, key: Key<'_>) -> Option<Vec<u8>> {
        let key = name(key);
        if key == GRAPH {
            return self.graph().given.take();
        }
        self.get_key(&key, true)
    }

    fn put(&self, key: Key<'_>, bytes: &[u8]) {
        let key = name(key);
        if key == GRAPH {
            return self.put_durable_owned(Key::Doc("graph"), bytes.to_vec());
        }
        self.put_key(&key, bytes, true);
    }

    fn put_durable_owned(&self, key: Key<'_>, bytes: Vec<u8>) {
        if name(key) != GRAPH {
            return self.put(key, &bytes);
        }
        if !self.ended.load(Ordering::SeqCst) {
            SAVED.with(|s| *s.borrow_mut() = Some(bytes));
        }
    }

    fn remove(&self, key: Key<'_>) {
        if name(key) == GRAPH {
            let mut g = self.graph();
            g.given = None;
            g.dropped = true;
        }
    }

    fn format(&self, name: &str) -> Option<([u8; 32], Vec<u8>)> {
        let mut b = self.get_key(&format!("format/{name}"), false)?;
        let key: [u8; 32] = b.get(..32)?.try_into().ok()?;
        b.drain(..32);
        Some((key, b))
    }

    fn put_format(&self, name: &str, key: &[u8; 32], bytes: &[u8]) {
        self.put_key(
            &format!("format/{name}"),
            &[&key[..], bytes].concat(),
            false,
        );
    }
}
