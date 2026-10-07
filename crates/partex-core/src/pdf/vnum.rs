//! Virtual object numbers (machine mode, `DESIGN.md` §7.6 "Numbering
//! is symbolic").
//!
//! pdfTeX numbers objects in allocation order, object streams included
//! (each takes the next number when its first object is written). An
//! object added early therefore renumbers every later object and moves
//! every later object stream's boundary, and a machine whose state held
//! those numbers would re-run everything after an added `\ref`.
//!
//! In machine mode (`ObjTab::virt`) the engine gives each object a
//! *virtual id* instead: a hash of the position of the step that made it
//! and a count within the step, the same in two runs for an object made
//! at the same place. The state holds only virtual ids. What pdfTeX's
//! numbers depend on is kept as a log of [`NumEvent`]s (an accumulating
//! cell, `MCell::Numbering`), and [`Numbering`] replays it: the link to
//! write the file's numbers, and the engine where TeX observes a number
//! (`\pdflastobj` and friends, a number given back, the end of the job),
//! which reads the log and so guards on it.

use alloc::collections::BTreeMap;

use super::out::PDF_OS_MAX_OBJS;

/// What pdfTeX's numbering depends on, in program order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NumEvent {
    /// Object `v` is created (`pdf_create_obj`): the next number.
    Create(i32),
    /// An object is written to an object stream (`pdf_os_prepare_obj`):
    /// with no stream open, a new one takes the next number.
    Start,
    /// That object ends (`pdf_end_obj`, `pdf_end_dict`): the hundredth
    /// closes its stream.
    End,
    /// The open stream, if any, is closed (the end of the job).
    Flush,
}

partex_engine::persist_enum!(NumEvent {
    Create(a0),
    Start,
    End,
    Flush
});

/// What a [`NumEvent`] did to the object streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Nothing,
    /// A stream opened with this number.
    Opened(i32),
    /// The open stream (this number) closed.
    Closed(i32),
}

/// The counters of the numbering, without its names: what decides the
/// numbers the next events take (a machine's `MCell::NumState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Counters {
    pub sys: i32,
    pub obj_ptr: i32,
    pub cur: i32,
    pub idx: i32,
    pub streams: i32,
}

impl Counters {
    /// The counters of `n`.
    #[must_use]
    pub fn of(n: &Numbering) -> Self {
        Self {
            sys: n.sys,
            obj_ptr: n.obj_ptr,
            cur: n.cur,
            idx: n.idx,
            streams: n.streams,
        }
    }

    /// Step over one event as [`Numbering::step`] does (the same
    /// counters: the unit test checks it), returning the number an object
    /// it creates takes, and whether it opened a stream.
    pub fn step(&mut self, e: NumEvent) -> (Option<i32>, bool) {
        match e {
            NumEvent::Create(_) => {
                self.sys += 1;
                self.obj_ptr = self.sys;
                (Some(self.sys), false)
            }
            NumEvent::Start => {
                if self.cur == 0 {
                    self.sys += 1;
                    self.cur = self.sys;
                    self.idx = 0;
                    self.streams += 1;
                    (None, true)
                } else {
                    self.idx += 1;
                    (None, false)
                }
            }
            NumEvent::End => {
                if self.cur != 0 && self.idx == PDF_OS_MAX_OBJS - 1 {
                    self.cur = 0;
                }
                (None, false)
            }
            NumEvent::Flush => {
                self.cur = 0;
                (None, false)
            }
        }
    }
}

/// pdfTeX's numbering, replayed from the events.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Numbering {
    /// Virtual id to pdfTeX's number, and back.
    pub num: BTreeMap<i32, i32>,
    pub vid: BTreeMap<i32, i32>,
    /// `sys_obj_ptr`, and `obj_ptr` (the last object users see).
    pub sys: i32,
    pub obj_ptr: i32,
    /// The open stream's number (0: none), its last object's index, and
    /// the streams opened (`pdf_os_cntr`).
    pub cur: i32,
    pub idx: i32,
    pub streams: i32,
    /// Events replayed.
    pub seen: usize,
}

impl Numbering {
    /// Replay one event.
    pub fn step(&mut self, e: NumEvent) -> Stream {
        self.seen += 1;
        match e {
            NumEvent::Create(v) => {
                self.sys += 1;
                self.obj_ptr = self.sys;
                self.num.insert(v, self.sys);
                self.vid.insert(self.sys, v);
                Stream::Nothing
            }
            NumEvent::Start => {
                if self.cur == 0 {
                    self.sys += 1;
                    self.cur = self.sys;
                    self.idx = 0;
                    self.streams += 1;
                    Stream::Opened(self.cur)
                } else {
                    self.idx += 1;
                    Stream::Nothing
                }
            }
            NumEvent::End => {
                if self.cur != 0 && self.idx == PDF_OS_MAX_OBJS - 1 {
                    let c = self.cur;
                    self.cur = 0;
                    Stream::Closed(c)
                } else {
                    Stream::Nothing
                }
            }
            NumEvent::Flush => {
                if self.cur == 0 {
                    Stream::Nothing
                } else {
                    let c = self.cur;
                    self.cur = 0;
                    Stream::Closed(c)
                }
            }
        }
    }

    /// Replay `events[self.seen..]`.
    pub fn catch_up(&mut self, events: &[NumEvent]) {
        for &e in events.get(self.seen..).unwrap_or(&[]) {
            self.step(e);
        }
    }

    /// pdfTeX's number of virtual id `v` (0 stays 0; an id never made is
    /// its own number, as a number TeX gave back that names no object).
    #[must_use]
    pub fn of(&self, v: i32) -> i32 {
        if v == 0 {
            0
        } else {
            self.num.get(&v).copied().unwrap_or(v)
        }
    }
}

/// A numbering to print pdfTeX's numbers by (`PdfOut::objnum`, where
/// the job observes them: its end). Not state: equal to any other,
/// saved as nothing.
#[derive(Clone, Debug, Default)]
pub struct Forced(pub Option<alloc::sync::Arc<Numbering>>);

impl PartialEq for Forced {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Forced {}

impl partex_engine::persist::Persist for Forced {
    fn save(&self, _: &mut partex_engine::persist::Saver) {}
    fn load(_: &mut partex_engine::persist::Loader) -> Option<Self> {
        Some(Self(None))
    }
}

/// Events per shared chunk of a [`Log`].
const CHUNK: usize = 1024;

/// The event log: full chunks shared between clones (a snapshot holds
/// the log as it was), and the chunk being filled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Log {
    full: alloc::vec::Vec<alloc::sync::Arc<[NumEvent]>>,
    tail: alloc::vec::Vec<NumEvent>,
    /// A running hash of every event (the cell's version).
    pub hash: u128,
    /// The counters after every event (`MCell::NumState`'s version).
    pub counters: Counters,
}

impl Log {
    pub fn push(&mut self, e: NumEvent) {
        use core::hash::Hash;
        let mut h = partex_engine::stablehash::StableHasher::new();
        (self.hash, e).hash(&mut h);
        self.hash = h.finish128();
        self.counters.step(e);
        self.tail.push(e);
        if self.tail.len() == CHUNK {
            let t = core::mem::take(&mut self.tail);
            self.full.push(t.into());
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.full.len() * CHUNK + self.tail.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The events from `from` on.
    #[must_use]
    pub fn since(&self, from: usize) -> alloc::vec::Vec<NumEvent> {
        let mut out = alloc::vec::Vec::new();
        for (c, chunk) in self.full.iter().enumerate() {
            let at = c * CHUNK;
            if at + CHUNK > from {
                out.extend_from_slice(&chunk[from.saturating_sub(at)..]);
            }
        }
        let at = self.full.len() * CHUNK;
        out.extend_from_slice(&self.tail[from.saturating_sub(at).min(self.tail.len())..]);
        out
    }

    /// Replay what `n` has not seen yet.
    pub fn catch_up(&self, n: &mut Numbering) {
        for e in self.since(n.seen) {
            n.step(e);
        }
    }

    /// pdfTeX's number of virtual id `v`, from `n` (a numbering of a
    /// prefix of this log) and the events after it, without changing `n`
    /// (as [`Numbering::of`]).
    #[must_use]
    pub fn final_of(&self, n: &Numbering, v: i32) -> i32 {
        if v == 0 {
            return 0;
        }
        if let Some(&k) = n.num.get(&v) {
            return k;
        }
        let mut c = Counters::of(n);
        for e in self.since(n.seen) {
            let (k, _) = c.step(e);
            if e == NumEvent::Create(v) {
                return k.unwrap_or(v);
            }
        }
        v
    }

    /// The object pdfTeX numbers `k`, from `n` and the events after it
    /// (as `n.vid.get(&k)` once caught up).
    #[must_use]
    pub fn vid_of(&self, n: &Numbering, k: i32) -> Option<i32> {
        if k <= n.sys {
            return n.vid.get(&k).copied();
        }
        let mut c = Counters::of(n);
        for e in self.since(n.seen) {
            let (got, _) = c.step(e);
            if c.sys == k {
                return match (got, e) {
                    (Some(_), NumEvent::Create(v)) => Some(v),
                    _ => None,
                };
            }
            if c.sys > k {
                return None;
            }
        }
        None
    }
}

partex_engine::persist_struct!(Counters {
    sys,
    obj_ptr,
    cur,
    idx,
    streams
});

/// Saved as it is kept: its full chunks shared (`persist::save_seq`: a
/// snapshot's log costs the chunks filled since the last), the chunk
/// being filled, the hash and the counters.
impl partex_engine::persist::Persist for Log {
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        partex_engine::persist::save_seq(&self.full, s);
        self.tail.save(s);
        self.hash.save(s);
        self.counters.save(s);
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        let full: alloc::vec::Vec<alloc::sync::Arc<[NumEvent]>> =
            partex_engine::persist::load_seq(l)?;
        let tail: alloc::vec::Vec<NumEvent> = partex_engine::persist::Persist::load(l)?;
        if full.iter().any(|c| c.len() != CHUNK) || tail.len() >= CHUNK {
            return None;
        }
        Some(Self {
            full,
            tail,
            hash: partex_engine::persist::Persist::load(l)?,
            counters: partex_engine::persist::Persist::load(l)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Counters, Log, NumEvent, Numbering};
    use alloc::vec::Vec;

    /// A log of these events, and the numbering it gives.
    fn log(events: &[NumEvent]) -> (Log, Numbering) {
        let mut l = Log::default();
        for &e in events {
            l.push(e);
        }
        let mut n = Numbering::default();
        l.catch_up(&mut n);
        (l, n)
    }

    /// A pseudo-random mix of creations and stream events.
    fn events(seed: u64, len: usize) -> Vec<NumEvent> {
        let mut x = seed.wrapping_mul(6_364_136_223_846_793_005) | 1;
        (0..len)
            .map(|i| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                match x % 7 {
                    0..3 => NumEvent::Create(i32::try_from(i + 10).unwrap()),
                    3 | 4 => NumEvent::Start,
                    5 => NumEvent::End,
                    _ => NumEvent::Flush,
                }
            })
            .collect()
    }

    /// The counters a log keeps are the numbering's, and the answers
    /// read from any cached prefix are those of the numbering caught up.
    #[test]
    fn counters_and_answers_are_the_numberings() {
        for seed in 1..40 {
            let ev = events(seed, 300);
            let (l, n) = log(&ev);
            assert_eq!(l.counters, Counters::of(&n), "seed {seed}");
            for cut in [0, 1, 57, 150, 299] {
                let (_, prefix) = log(&ev[..cut]);
                for &e in &ev {
                    if let NumEvent::Create(v) = e {
                        assert_eq!(l.final_of(&prefix, v), n.of(v), "seed {seed} cut {cut}");
                    }
                }
                assert_eq!(l.final_of(&prefix, 99_999), 99_999);
                for k in 0..=n.sys + 2 {
                    assert_eq!(
                        l.vid_of(&prefix, k),
                        n.vid.get(&k).copied(),
                        "seed {seed} k {k}"
                    );
                }
            }
        }
    }

    /// An object made before `k` changes `k`'s number: a guard on the
    /// answer fails, and the region re-runs. An earlier object renamed
    /// (another virtual id, the same order) changes the whole numbering's
    /// version but not the answer: the region replays.
    #[test]
    fn an_answer_changes_with_an_insertion_not_with_a_name() {
        let old = [
            NumEvent::Create(1),
            NumEvent::Create(2),
            NumEvent::Create(7),
        ];
        let inserted = [
            NumEvent::Create(1),
            NumEvent::Create(5),
            NumEvent::Create(2),
            NumEvent::Create(7),
        ];
        let renamed = [
            NumEvent::Create(1),
            NumEvent::Create(9),
            NumEvent::Create(7),
        ];
        let (a, na) = log(&old);
        let (b, nb) = log(&inserted);
        let (c, nc) = log(&renamed);
        let zero = Numbering::default();
        assert_ne!(a.final_of(&zero, 7), b.final_of(&zero, 7), "insertion");
        assert_eq!(a.final_of(&zero, 7), c.final_of(&zero, 7), "a name");
        assert_ne!(a.hash, c.hash, "the whole numbering differs");
        assert_eq!(na.of(7), 3);
        assert_eq!(nb.of(7), 4);
        assert_eq!(nc.of(7), 3);
        // (and the object numbered 3 is the same one, or not)
        assert_eq!(a.vid_of(&zero, 3), c.vid_of(&zero, 3));
        assert_ne!(a.vid_of(&zero, 3), b.vid_of(&zero, 3));
        // (the counters, which number what a region makes, follow the
        // count only)
        assert_eq!(a.counters, c.counters);
        assert_ne!(a.counters, b.counters);
    }
}
