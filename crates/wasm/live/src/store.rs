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
}

impl Kept {
    pub fn new(ask: Ask, put: Put) -> Kept {
        Kept {
            slots: Mutex::default(),
            answered: Condvar::new(),
            ask,
            put,
        }
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
        self.get_key(&name(key), true)
    }

    fn put(&self, key: Key<'_>, bytes: &[u8]) {
        self.put_key(&name(key), bytes, true);
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
