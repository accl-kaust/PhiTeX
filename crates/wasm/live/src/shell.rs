//! `\write18` through the host: the engine's [`ShellHook`]
//! (`docs/note-shell-hook.md`). The engine decides what runs (restricted
//! mode's list, web2c's quoting) and over which files; the host runs it
//! (latexminted in Pyodide, on its thread 0, which has the runner loaded).
//!
//! `run` is called on an engine thread, with no engine lock held: it
//! posts the call to the host (`phitex.shell`, framed below) and waits on
//! a condition variable until thread 0 answers (`ph_shell_done`). Calls
//! from several threads are answered in any order, each by its number.
//!
//! The frames (little-endian): a call is `u32 n, command, files`; files
//! are `u32 count, (u32 n, name, u32 n, bytes) x count`; an answer is
//! `i32 status, files written, files removed` (names, no bytes).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use phitex_tex::{ShellHook, ShellRan};

/// Where a call is posted to the host: its number and its frame.
pub type Post = fn(u32, &[u8]);

pub struct Hook {
    post: Post,
    next: AtomicU32,
    answers: Mutex<HashMap<u32, Option<ShellRan>>>,
    answered: Condvar,
}

impl Hook {
    pub fn new(post: Post) -> Hook {
        Hook {
            post,
            next: AtomicU32::new(1),
            answers: Mutex::default(),
            answered: Condvar::new(),
        }
    }

    /// The host's answer to call `id` (its frame; none if it is not one:
    /// the command is taken for not found).
    pub fn done(&self, id: u32, frame: &[u8]) {
        let ran = answer(frame).unwrap_or(ShellRan {
            status: 127 << 8,
            wrote: Vec::new(),
            removed: Vec::new(),
            read: None,
            stdout: Vec::new(),
        });
        let mut a = self.answers.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(slot) = a.get_mut(&id) {
            *slot = Some(ran);
        }
        drop(a);
        self.answered.notify_all();
    }
}

impl ShellHook for Hook {
    fn run(&self, command: &str, files: &[(String, Arc<[u8]>)]) -> ShellRan {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut frame = Vec::new();
        put(&mut frame, command.as_bytes());
        put_files(&mut frame, files.iter().map(|(n, b)| (n.as_str(), &**b)));
        self.answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, None);
        // (the lock let go: a host without threads answers inside the call)
        (self.post)(id, &frame);
        let mut a = self.answers.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(Some(_)) = a.get(&id) {
                return a.remove(&id).flatten().expect("answered");
            }
            if !crate::THREADED {
                // (no thread to answer later: not answered in the call, not found)
                a.remove(&id);
                return ShellRan {
                    status: 127 << 8,
                    wrote: Vec::new(),
                    removed: Vec::new(),
                    read: None,
                    stdout: Vec::new(),
                };
            }
            a = self
                .answered
                .wait(a)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

fn put(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&u32::try_from(b.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(b);
}

fn put_files<'a>(out: &mut Vec<u8>, files: impl ExactSizeIterator<Item = (&'a str, &'a [u8])>) {
    out.extend_from_slice(&u32::try_from(files.len()).unwrap_or(u32::MAX).to_le_bytes());
    for (name, bytes) in files {
        put(out, name.as_bytes());
        put(out, bytes);
    }
}

/// A frame's reader.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn u32(&mut self) -> Option<u32> {
        let (w, rest) = self.0.split_first_chunk::<4>()?;
        self.0 = rest;
        Some(u32::from_le_bytes(*w))
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = usize::try_from(self.u32()?).ok()?;
        let (b, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(b)
    }

    fn files(&mut self) -> Option<Vec<(String, Vec<u8>)>> {
        (0..self.u32()?)
            .map(|_| {
                Some((
                    String::from_utf8(self.bytes()?.to_vec()).ok()?,
                    self.bytes()?.to_vec(),
                ))
            })
            .collect()
    }
}

fn answer(frame: &[u8]) -> Option<ShellRan> {
    let mut r = Reader(frame);
    let status = r.u32()?.cast_signed();
    let wrote = r.files()?;
    let removed = r.files()?.into_iter().map(|(n, _)| n).collect();
    Some(ShellRan {
        status,
        wrote,
        removed,
        read: None,
        stdout: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_frame_reads_back() {
        let mut f = Vec::new();
        f.extend_from_slice(&(256i32).to_le_bytes());
        put_files(
            &mut f,
            [("_minted/a.highlight.minted", &b"x"[..])].into_iter(),
        );
        put_files(&mut f, [("old.data.minted", &b""[..])].into_iter());
        let r = answer(&f).expect("a frame");
        assert_eq!(r.status, 256);
        assert_eq!(
            r.wrote,
            [("_minted/a.highlight.minted".to_owned(), b"x".to_vec())]
        );
        assert_eq!(r.removed, ["old.data.minted"]);
        assert!(answer(&f[..f.len() - 1]).is_none());
    }
}
