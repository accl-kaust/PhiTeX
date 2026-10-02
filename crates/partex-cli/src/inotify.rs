//! Which watched directories changed since the last look (Linux inotify):
//! what lets [`crate::native::NativeHost`]'s `unchanged` skip the `stat` of
//! every input a rebuild loaded (DESIGN 7.17.3, "A rebuild's file checks
//! cost the files that changed"). A directory is watched for its entries
//! (made, removed, renamed) and for writes to and attribute changes of
//! the files in it; a load whose directories had none since they were
//! watched is as the last check found it. `PARTEX_INOTIFY=0`: no watcher,
//! every load checked by its stamps.
#![expect(unsafe_code, reason = "FFI to the C library's inotify")]

use std::collections::HashMap;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::raw::{c_char, c_int, c_void};

unsafe extern "C" {
    fn inotify_init1(flags: c_int) -> c_int;
    fn inotify_add_watch(fd: c_int, path: *const c_char, mask: u32) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
}

const IN_NONBLOCK: c_int = 0o4000;
const IN_CLOEXEC: c_int = 0o200_0000;
const IN_MODIFY: u32 = 0x2;
const IN_ATTRIB: u32 = 0x4;
const IN_CLOSE_WRITE: u32 = 0x8;
const IN_MOVED_FROM: u32 = 0x40;
const IN_MOVED_TO: u32 = 0x80;
const IN_CREATE: u32 = 0x100;
const IN_DELETE: u32 = 0x200;
const IN_DELETE_SELF: u32 = 0x400;
const IN_MOVE_SELF: u32 = 0x800;
const IN_Q_OVERFLOW: u32 = 0x4000;
const IN_IGNORED: u32 = 0x8000;

pub struct Watcher {
    fd: OwnedFd,
    /// Each watched directory, as named, and each watch's names (one
    /// directory named two ways is one watch).
    dirs: HashMap<Vec<u8>, c_int>,
    names: HashMap<c_int, Vec<Vec<u8>>>,
    /// The events seen, counted: the last one of each name in each
    /// directory, and when each directory was watched from; `lost`: when
    /// events were last lost (the queue overflowed), before which nothing
    /// is known.
    seq: u64,
    last: HashMap<Vec<u8>, HashMap<Vec<u8>, u64>>,
    since: HashMap<Vec<u8>, u64>,
    lost: u64,
}

impl Watcher {
    /// A watcher, unless `PARTEX_INOTIFY=0` or the system has none.
    pub fn new() -> Option<Self> {
        if std::env::var("PARTEX_INOTIFY").is_ok_and(|v| v == "0") {
            return None;
        }
        // SAFETY: inotify_init1 takes flags and returns a new descriptor or -1.
        let fd = unsafe { inotify_init1(IN_NONBLOCK | IN_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        Some(Watcher {
            // SAFETY: `fd` is a descriptor just made and owned by no one else.
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
            dirs: HashMap::new(),
            names: HashMap::new(),
            seq: 1,
            last: HashMap::new(),
            since: HashMap::new(),
            lost: 0,
        })
    }

    /// The events so far, counted: a check made now knows of them all.
    pub fn now(&self) -> u64 {
        self.seq
    }

    /// Watch directory `dir` from now on, if it is not watched already
    /// (and can be: a directory that is not there is never watched).
    pub fn watch(&mut self, dir: &[u8]) {
        if self.dirs.contains_key(dir) {
            return;
        }
        let Ok(c) = std::ffi::CString::new(dir) else {
            return;
        };
        let mask = IN_MODIFY
            | IN_ATTRIB
            | IN_CLOSE_WRITE
            | IN_MOVED_FROM
            | IN_MOVED_TO
            | IN_CREATE
            | IN_DELETE
            | IN_DELETE_SELF
            | IN_MOVE_SELF;
        // SAFETY: the descriptor is ours and `c` a NUL-terminated path.
        let wd = unsafe { inotify_add_watch(self.fd.as_raw_fd(), c.as_ptr(), mask) };
        if wd >= 0 {
            self.dirs.insert(dir.to_vec(), wd);
            self.names.entry(wd).or_default().push(dir.to_vec());
            self.since.insert(dir.to_vec(), self.seq);
        }
    }

    /// Whether directory `dir` has been watched since `check` (a count of
    /// [`Watcher::now`]) with no event since of a name `names` gives:
    /// what a check then found of those files still holds (a file made,
    /// written, renamed or removed is an event of its name; the other
    /// files of the directory are not looked at).
    pub fn quiet<'a>(
        &self,
        dir: &[u8],
        names: impl IntoIterator<Item = &'a [u8]>,
        check: u64,
    ) -> bool {
        // (events counted up to `check` were taken before that check)
        if self.lost > check || self.since.get(dir).is_none_or(|&s| s > check) {
            return false;
        }
        let Some(last) = self.last.get(dir) else {
            return true;
        };
        names
            .into_iter()
            .all(|n| last.get(n).is_none_or(|&l| l <= check))
    }

    /// Take the events queued since the last look.
    pub fn look(&mut self) {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            // SAFETY: the descriptor is ours, and `buf` is writable for its length.
            let n = unsafe { read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            let Ok(n) = usize::try_from(n) else {
                match std::io::Error::last_os_error().kind() {
                    std::io::ErrorKind::WouldBlock => break,
                    std::io::ErrorKind::Interrupted => continue,
                    _ => {
                        self.seq += 1;
                        self.lost = self.seq;
                        break;
                    }
                }
            };
            if n == 0 {
                break;
            }
            let word = |i: usize| u32::from_ne_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
            let mut i = 0;
            while i + 16 <= n {
                let wd = c_int::from_ne_bytes(word(i).to_ne_bytes());
                let mask = word(i + 4);
                let len = word(i + 12) as usize;
                let raw = &buf[(i + 16).min(n)..(i + 16 + len).min(n)];
                let name = &raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())];
                i += 16 + len;
                self.seq += 1;
                if mask & IN_Q_OVERFLOW != 0 {
                    self.lost = self.seq;
                    continue;
                }
                let Some(dirs) = self.names.get(&wd) else {
                    continue;
                };
                for d in dirs {
                    self.last
                        .entry(d.clone())
                        .or_default()
                        .insert(name.to_vec(), self.seq);
                }
                if mask & (IN_DELETE_SELF | IN_MOVE_SELF | IN_IGNORED) != 0 {
                    // (the directory itself moved or went: watched no more,
                    // so never quiet until watched again)
                    for d in self.names.remove(&wd).unwrap_or_default() {
                        self.dirs.remove(&d);
                        self.since.remove(&d);
                    }
                }
            }
        }
    }
}

/// A path's directory and name, as the watcher names them (a bare name
/// is in `.`).
pub fn split(p: &[u8]) -> (&[u8], &[u8]) {
    match p.iter().rposition(|&b| b == b'/') {
        Some(0) => (&p[..1], &p[1..]),
        Some(i) => (&p[..i], &p[i + 1..]),
        None => (b".", p),
    }
}
