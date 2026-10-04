//! Virtual object numbers resolved per step, for [`super::Splice`]
//! (DESIGN 3.8).
//!
//! With virtual numbers (`pdf/vnum.rs`) a chunk's bytes depend on the
//! numbering of the whole job, replayed from every chunk's `Num` events,
//! and on the fonts' order (`FontLoad`). [`super::link_cached`]
//! resolves them at every link, then lays out every file again.
//!
//! [`Resolver`] keeps what the numbering needs from each step: the
//! counters and the object-stream file at its entry, and a hash of the
//! events that move them (its *signature*: `Num`, `FontLoad`,
//! `ObjStmStart`, in order). Two cases:
//!
//! - **Fast path.** Every step that changed has the signature it had
//!   (a step new to the build or gone from it has an empty one). Then the
//!   job's numbering, the fonts' numbers and every other step's entry are
//!   what they were. Only the changed steps' chunks are resolved, from
//!   their entries. They go to the splice as chunks with real numbers, and
//!   it lays out what they reach.
//! - **Full path.** Any signature differs at all. The numbering is made
//!   again from every step, and every chunk is resolved. The splice then
//!   lays out from cold.
//!
//! Either way the files are [`super::link`]'s, byte for byte.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::hash::Hash;

use super::{Effect, StepChunks, numbering_of, region_inputs, resolve_region};
use crate::host::WriteId;
use crate::pdf::vnum::{Counters, Numbering};
use crate::pdf::xref::Deflate;
use partex_engine::stablehash::StableHasher;

/// A step's entry: the numbering's counters, and the file whose object
/// stream is being filled.
type Entry = (Counters, Option<WriteId>);

/// A step's chunks, each with its version.
type Chunks = Vec<(u128, Arc<[Effect]>)>;

/// The signature of a step with no events that move the numbering.
fn empty_sig() -> u128 {
    StableHasher::new().finish128()
}

/// The signature of `chunks` (module doc): the events, in order, that
/// move the numbering, the fonts' order or the object-stream file.
fn signature<'a>(chunks: impl Iterator<Item = &'a [Effect]>) -> u128 {
    let mut h = StableHasher::new();
    for fx in chunks {
        for e in fx {
            match e {
                Effect::Num(ev, _) => {
                    0u8.hash(&mut h);
                    ev.hash(&mut h);
                }
                Effect::FontLoad(f) => {
                    1u8.hash(&mut h);
                    f.hash(&mut h);
                }
                Effect::ObjStmStart { file, .. } => {
                    2u8.hash(&mut h);
                    file.0.hash(&mut h);
                }
                _ => {}
            }
        }
    }
    h.finish128()
}

/// A chunk as last resolved: its own version, its entry, the numbers it
/// writes ([`region_inputs`]), and its resolved effects with their
/// version.
#[derive(Clone, Debug)]
struct Rec {
    raw: u128,
    entry: Entry,
    inputs: Vec<i32>,
    version: u128,
    out: Arc<[Effect]>,
    /// Its streams compressed, by the hash of their level and bytes.
    zs: Vec<u128>,
}

/// The resolver's state (module doc).
#[derive(Clone, Debug, Default)]
pub struct Resolver {
    /// The job's numbering and the fonts' numbers.
    numbering: Arc<(Numbering, BTreeMap<i32, i32>)>,
    /// Each step's entry, signature and number of chunks.
    steps: BTreeMap<u32, (Entry, u128, usize)>,
    /// Each chunk as last resolved, by step and place in it.
    chunks: BTreeMap<(u32, u32), Rec>,
    /// The live chunks' streams compressed, by the hash of their level
    /// and bytes: a chunk resolved again for numbers outside its streams
    /// (the numbering moved) finds its streams' bytes as they were.
    zs: BTreeMap<u128, Vec<u8>>,
    live: bool,
    /// What the last link took: the steps it gave the splice, whether
    /// the numbering was made again, and the chunks resolved again.
    pub steps_out: usize,
    pub full: bool,
    pub resolved: usize,
}

impl Resolver {
    /// Forget everything: the next resolution is a cold one.
    pub fn reset(&mut self) {
        *self = Resolver::default();
    }

    /// Whether a resolution was made, and holds.
    #[must_use]
    pub fn live(&self) -> bool {
        self.live
    }

    /// The fast path: the steps in `changes` resolved, if every one's
    /// signature is the one it had (the numbering holds); `None` if not,
    /// or if a stream's length and its stream are in different chunks.
    /// Then the caller resolves every step ([`Resolver::full`]).
    pub fn changes(
        &mut self,
        changes: &[StepChunks],
        deflate: &mut Deflate<'_>,
    ) -> Option<Vec<StepChunks>> {
        if !self.live {
            return None;
        }
        self.full = false;
        self.resolved = 0;
        let empty = empty_sig();
        for c in changes {
            let was = self.steps.get(&c.step).map_or(empty, |x| x.1);
            let now = c
                .chunks
                .as_ref()
                .map_or(empty, |v| signature(v.iter().map(|x| &x.1[..])));
            if was != now {
                return None;
            }
        }
        let numbering = self.numbering.clone();
        let mut out = Vec::with_capacity(changes.len());
        for c in changes {
            // (a step gone, or with no chunks: as a step not in the build)
            let Some(chunks) = c.chunks.as_ref().filter(|v| !v.is_empty()) else {
                self.steps.remove(&c.step);
                self.forget(c.step, 0);
                out.push(c.clone());
                continue;
            };
            let entry = if let Some(x) = self.steps.get(&c.step) {
                x.0
            } else {
                // (a step new to the build, its signature empty: its
                // entry moves nothing)
                let e = (Counters::default(), None);
                self.steps.insert(c.step, (e, empty, 0));
                e
            };
            let (resolved, _) = self.resolve_from(c.step, chunks, entry, &numbering, deflate)?;
            self.forget(c.step, chunks.len());
            if let Some(x) = self.steps.get_mut(&c.step) {
                x.2 = chunks.len();
            }
            out.push(StepChunks {
                step: c.step,
                order: c.order,
                chunks: Some(resolved),
            });
        }
        if self.zs.len() > 2 * self.chunks.len() + 256 {
            self.collect();
        }
        self.steps_out = out.len();
        Some(out)
    }

    /// The full path: `steps`, every live step in program order, with
    /// the numbering made again from them all. Each chunk whose own
    /// version, entry and written numbers are as they were is taken as
    /// last resolved; the steps given back are those with a chunk
    /// resolved again, or gone. `None` if a stream's length and its stream
    /// are in different chunks (the caller links in full, unresolved
    /// here). The flag: the resolver was not live, so the splice lays
    /// out from cold.
    pub fn full(
        &mut self,
        steps: &[StepChunks],
        deflate: &mut Deflate<'_>,
    ) -> Option<(Vec<StepChunks>, bool)> {
        let cold = !self.live;
        self.live = false;
        self.full = true;
        self.resolved = 0;
        let all: Vec<&[Effect]> = steps
            .iter()
            .flat_map(|c| c.chunks.iter().flatten().map(|x| &x.1[..]))
            .collect();
        let numbering = Arc::new(numbering_of(&all)?);
        let mut old_steps = core::mem::take(&mut self.steps);
        let mut entry: Entry = (Counters::default(), None);
        let mut out = Vec::new();
        for c in steps {
            let chunks = c.chunks.as_deref().unwrap_or_default();
            let sig = signature(chunks.iter().map(|x| &x.1[..]));
            let was = old_steps.remove(&c.step).map(|x| x.2);
            self.steps.insert(c.step, (entry, sig, chunks.len()));
            let before = self.resolved;
            let (resolved, exit) = self.resolve_from(c.step, chunks, entry, &numbering, deflate)?;
            self.forget(c.step, chunks.len());
            entry = exit;
            if was != Some(chunks.len()) || self.resolved > before {
                out.push(StepChunks {
                    step: c.step,
                    order: c.order,
                    chunks: Some(resolved),
                });
            }
        }
        // (the steps gone from the build)
        for (step, _) in old_steps {
            self.forget(step, 0);
            out.push(StepChunks {
                step,
                order: 0,
                chunks: None,
            });
        }
        self.numbering = numbering;
        self.live = true;
        self.collect();
        self.steps_out = out.len();
        Some((out, cold))
    }

    /// The compressed streams no live chunk holds dropped.
    fn collect(&mut self) {
        let live: alloc::collections::BTreeSet<u128> = self
            .chunks
            .values()
            .flat_map(|r| r.zs.iter().copied())
            .collect();
        self.zs.retain(|k, _| live.contains(k));
    }

    /// The records of step `step`'s chunks from place `from` on dropped.
    fn forget(&mut self, step: u32, from: usize) {
        let from = u32::try_from(from).unwrap_or(u32::MAX);
        let gone: Vec<(u32, u32)> = self
            .chunks
            .range((step, from)..=(step, u32::MAX))
            .map(|(k, _)| *k)
            .collect();
        for k in gone {
            self.chunks.remove(&k);
        }
    }

    /// Step `step`'s chunks resolved from `entry`, and the entry after
    /// them. A chunk whose own version, entry and written numbers are as
    /// last resolved is taken as it was (its version too, so the splice
    /// passes over it); else resolved, its version its own with its entry
    /// and numbers.
    fn resolve_from(
        &mut self,
        step: u32,
        chunks: &[(u128, Arc<[Effect]>)],
        entry: Entry,
        (n, fonts): &(Numbering, BTreeMap<i32, i32>),
        deflate: &mut Deflate<'_>,
    ) -> Option<(Chunks, Entry)> {
        let (c, mut file) = entry;
        let mut again = Numbering {
            sys: c.sys,
            obj_ptr: c.obj_ptr,
            cur: c.cur,
            idx: c.idx,
            streams: c.streams,
            ..Numbering::default()
        };
        let mut out = Vec::with_capacity(chunks.len());
        for (k, (v, fx)) in chunks.iter().enumerate() {
            let key = (step, u32::try_from(k).unwrap_or(u32::MAX));
            let at: Entry = (Counters::of(&again), file);
            let inputs = region_inputs(fx, n, fonts);
            if let Some(r) = self.chunks.get(&key)
                && r.raw == *v
                && r.entry == at
                && r.inputs == inputs
            {
                // (the counters and the file move on as its resolution
                // did)
                for e in fx.iter() {
                    match e {
                        Effect::Num(ev, _) => {
                            again.step(*ev);
                        }
                        Effect::ObjStmStart { file: f, .. } => file = Some(*f),
                        _ => {}
                    }
                }
                out.push((r.version, r.out.clone()));
                continue;
            }
            let mut used = Vec::new();
            let zs = &mut self.zs;
            let mut z = |level: i32, data: &[u8]| -> Option<Vec<u8>> {
                let key = StableHasher::of(&(level, data));
                used.push(key);
                if let Some(b) = zs.get(&key) {
                    return Some(b.clone());
                }
                let b = deflate(level, data)?;
                zs.insert(key, b.clone());
                Some(b)
            };
            let (r, pending) = resolve_region(fx.iter(), (n, fonts), &mut again, &mut file, &mut z);
            if pending {
                self.reset();
                return None;
            }
            let version = StableHasher::of(&(*v, at.0, at.1.map(|f| f.0), &inputs));
            let r: Arc<[Effect]> = Arc::from(r);
            out.push((version, r.clone()));
            self.chunks.insert(
                key,
                Rec {
                    raw: *v,
                    entry: at,
                    inputs,
                    version,
                    out: r,
                    zs: used,
                },
            );
            self.resolved += 1;
        }
        Some((out, (Counters::of(&again), file)))
    }
}
