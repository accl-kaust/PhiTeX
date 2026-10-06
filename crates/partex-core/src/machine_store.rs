//! A machine-mode build kept outside the process (DESIGN.md §7.9,
//! "Persisted machine builds"): the build's regions, their snapshots and
//! the starting and final states as [`Persist`] values, for a store of
//! content-addressed blobs ([`Saver::merkle`]).
//!
//! A snapshot is the engine ([`Tex::save_state`] and
//! [`Tex::save_machine_extras`]), what its host keeps of its own
//! ([`StoreHost`]) and its node lists in their shared chunks; the chunks
//! shared between snapshots (eqtb, token lists, node lists, files) are
//! shared values, so the store keeps each once. A value a region wrote
//! holds its snapshot by reference, each snapshot saved once.
//!
//! Loading needs a template host: the parts every clone of the host
//! shares (the native host, the files served) are the new process's,
//! given back to each state loaded.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use partex_engine::persist::{Loader, Persist, Saver};
use partex_incr::{Build, Trace, Version};

use super::{
    CellHost, CellTracker, CellValue, FontSlot, Held, LazyBody, Lists, MCell, MValue, Named,
    ObjRegion, PageValue, SnapBody, Snapshot, TexMachine, V,
};
use crate::effects::Effect;
use crate::pdf::objtab::NameCell;
use crate::tex::Tex;

/// A host whose own state can be kept with a machine's snapshots.
pub trait StoreHost: CellHost {
    /// Save what this state of the host holds of its own (not what every
    /// clone shares).
    fn save_own(&self, s: &mut Saver);
    /// A host with this one's shared parts and the state [`save_own`]
    /// saved.
    ///
    /// [`save_own`]: StoreHost::save_own
    fn load_own(&self, l: &mut Loader) -> Option<Self>;
}

partex_engine::persist_enum!(MCell {
    Rest,
    Glyphs,
    Classes,
    File(a0),
    Line(a0, a1),
    Sealed(a0, a1),
    Written(a0),
    Eqtb(a0),
    Link(a0),
    Name(a0),
    Font(a0),
    FontName(a0),
    FontExpand(a0, a1),
    FontOrder,
    Obj(a0),
    Tree(a0, a1),
    Dests,
    Numbering,
    Positions,
    Page,
    PdfLast(a0),
    FinalNum(a0),
    OfFinal(a0),
    NumState,
    PdfWord(a0),
});

partex_engine::persist_struct!(PageValue { builder, list });

partex_engine::persist_struct!(super::Pos { key, file, line });

partex_engine::persist_enum!(NameCell { Tree(a0, a1) });

partex_engine::persist_struct!(ObjRegion {
    reads,
    writes,
    names_read,
    names_written,
    overflow
});

partex_engine::persist_struct!(FontSlot {
    ident,
    retagged,
    metrics,
    tfm,
    name,
    area,
    hyphen_char,
    skew_char,
    codes,
    expand,
    pdf,
    native,
    native_dir
});

partex_engine::persist_struct!(CellValue {
    word,
    level,
    named,
    name
});

partex_engine::persist_enum!(Named {
    Nothing,
    List {
        toks,
        protected,
        interned
    },
    Glue(a0, a1),
    Shape(a0),
    Box(a0),
});

/// Save an engine with the machine's extras and its host's own state;
/// `false` if it cannot be saved.
fn save_tex<H: StoreHost>(tex: &Tex<H, CellTracker>, s: &mut Saver) -> bool {
    tex.host.save_own(s);
    tex.save_state(s) && tex.save_machine_extras(s)
}

fn load_tex<H: StoreHost>(l: &mut Loader, template: &H) -> Option<Tex<H, CellTracker>> {
    let host = template.load_own(l)?;
    let mut tex = Tex::load_state(l, host, CellTracker::default())?;
    tex.load_machine_extras(l)?;
    Some(tex)
}

/// Save a snapshot (shared: each once; one from the store as a reference
/// to its blob, not loaded).
fn save_snapshot<H: StoreHost + 'static>(snap: &Arc<Snapshot<H>>, s: &mut Saver, ok: &mut bool) {
    if let Some(h) = snap.stored()
        && s.blob_ref(h)
    {
        return;
    }
    s.share(
        Arc::as_ptr(snap).cast::<()>() as usize,
        1,
        "snapshot",
        || partex_engine::persist::Pin::Send(alloc::boxed::Box::new(snap.clone())),
        |s| {
            *ok &= save_tex(&snap.tex, s);
            snap.lists.0.save(s);
            snap.started.save(s);
            snap.halted.save(s);
        },
    );
}

/// What loading a build needs besides the bytes: the template host, and
/// where snapshots in the store come from when they are to be loaded
/// only when first used (with those met so far).
pub struct Cx<'a, H: StoreHost> {
    template: &'a H,
    lazy: Option<MakeLazy<'a, H>>,
    snaps: core::cell::RefCell<BTreeMap<u128, Arc<Snapshot<H>>>>,
}

/// A snapshot in the store, by blob, to load when first used.
pub type MakeLazy<'a, H> = &'a (dyn Fn(u128) -> alloc::boxed::Box<dyn LazyBody<H>> + Sync);

impl<'a, H: StoreHost> Cx<'a, H> {
    #[must_use]
    pub fn new(template: &'a H, lazy: Option<MakeLazy<'a, H>>) -> Self {
        Self {
            template,
            lazy,
            snaps: core::cell::RefCell::default(),
        }
    }
}

/// A snapshot's contents, saved by [`save_snapshot`] as blob of its own
/// (loaded from that blob's bytes).
pub fn load_snapshot_body<H: StoreHost + 'static>(
    l: &mut Loader,
    template: &H,
) -> Option<SnapBody<H>> {
    let tex = load_tex(l, template)?;
    let lists = Lists(Persist::load(l)?);
    Some(SnapBody::new(
        tex,
        lists,
        Persist::load(l)?,
        Persist::load(l)?,
    ))
}

fn load_snapshot<H: StoreHost + 'static>(l: &mut Loader, cx: &Cx<H>) -> Option<Arc<Snapshot<H>>> {
    if let Some(make) = cx.lazy
        && let Some(h) = l.take_blob_ref()
    {
        let mut snaps = cx.snaps.borrow_mut();
        let s = snaps.entry(h).or_insert_with(|| {
            Arc::new(Snapshot {
                held: Held::Stored(h, make(h)),
            })
        });
        return Some(s.clone());
    }
    let s = l.share(|l| {
        Some(Arc::new(Snapshot::here(load_snapshot_body(
            l,
            cx.template,
        )?)))
    })?;
    l.note(Arc::as_ptr(&s).cast::<()>() as usize, 1, &s);
    Some(s)
}

/// Save a cell's value.
fn save_value<H: StoreHost + 'static>(v: &MValue<H>, s: &mut Saver, ok: &mut bool) {
    v.version.save(s);
    match &v.v {
        V::Version => s.enc.u8(0),
        V::Word(w) => {
            s.enc.u8(1);
            w.save(s);
        }
        V::Lazy(snap, p) => {
            s.enc.u8(2);
            save_snapshot(snap, s, ok);
            p.save(s);
        }
        V::Rest(snap) => {
            s.enc.u8(3);
            save_snapshot(snap, s, ok);
        }
        V::File(f) => {
            s.enc.u8(4);
            f.save(s);
        }
        V::Line => s.enc.u8(5),
        V::Glyphs(g) => {
            s.enc.u8(6);
            g.save(s);
        }
        V::Sealed(x) => {
            s.enc.u8(7);
            x.save(s);
        }
        V::Written(w) => {
            s.enc.u8(8);
            w.save(s);
        }
        V::Obj(e) => {
            s.enc.u8(9);
            e.save(s);
        }
        V::Tree(k) => {
            s.enc.u8(10);
            k.save(s);
        }
        V::Dests(d) => {
            s.enc.u8(11);
            d.save(s);
        }
        V::Numbering(d) => {
            s.enc.u8(12);
            d.save(s);
        }
        V::Int(k) => {
            s.enc.u8(13);
            k.save(s);
        }
        V::Font(x) => {
            s.enc.u8(14);
            x.save(s);
        }
        V::FontOrder(x) => {
            s.enc.u8(15);
            x.save(s);
        }
        V::Positions(p) => {
            s.enc.u8(16);
            p.save(s);
        }
        V::Page(p) => {
            s.enc.u8(17);
            p.save(s);
        }
    }
}

fn load_value<H: StoreHost + 'static>(l: &mut Loader, cx: &Cx<H>) -> Option<MValue<H>> {
    let version = u128::load(l)?;
    let v = match l.dec.u8()? {
        0 => V::Version,
        1 => V::Word(Persist::load(l)?),
        2 => {
            let snap = load_snapshot(l, cx)?;
            V::Lazy(snap, Persist::load(l)?)
        }
        3 => V::Rest(load_snapshot(l, cx)?),
        4 => V::File(Persist::load(l)?),
        5 => V::Line,
        6 => V::Glyphs(Persist::load(l)?),
        7 => V::Sealed(Persist::load(l)?),
        8 => V::Written(Persist::load(l)?),
        9 => V::Obj(Persist::load(l)?),
        10 => V::Tree(Persist::load(l)?),
        11 => V::Dests(Persist::load(l)?),
        12 => V::Numbering(Persist::load(l)?),
        13 => V::Int(Persist::load(l)?),
        14 => V::Font(Persist::load(l)?),
        15 => V::FontOrder(Persist::load(l)?),
        16 => V::Positions(Persist::load(l)?),
        17 => V::Page(Persist::load(l)?),
        _ => return None,
    };
    Some(MValue { version, v })
}

/// Save a region's trace.
fn save_trace<H: StoreHost + 'static>(t: &Trace<TexMachine<H>>, s: &mut Saver, ok: &mut bool) {
    t.entry.save(s);
    t.exit.save(s);
    t.guards.len().save(s);
    for (c, v) in &t.guards {
        c.save(s);
        v.0.save(s);
    }
    t.writes.len().save(s);
    for (c, v, ver) in &t.writes {
        c.save(s);
        save_value(v, s, ok);
        ver.0.save(s);
    }
    t.effects.save(s);
    t.holes.save(s);
    t.allocs.save(s);
    t.cost.save(s);
    t.born.save(s);
}

fn load_trace<H: StoreHost + 'static>(l: &mut Loader, cx: &Cx<H>) -> Option<Trace<TexMachine<H>>> {
    let entry = super::Pos::load(l)?;
    let exit = super::Pos::load(l)?;
    let n = usize::load(l)?;
    let mut guards = Vec::with_capacity(n.min(1 << 20));
    for _ in 0..n {
        guards.push((MCell::load(l)?, Version(u128::load(l)?)));
    }
    let n = usize::load(l)?;
    let mut writes = Vec::with_capacity(n.min(1 << 20));
    for _ in 0..n {
        let c = MCell::load(l)?;
        let v = load_value(l, cx)?;
        writes.push((c, v, Version(u128::load(l)?)));
    }
    Some(Trace {
        entry,
        exit,
        guards,
        writes,
        effects: Vec::<Effect>::load(l)?,
        holes: Persist::load(l)?,
        allocs: Persist::load(l)?,
        cost: Persist::load(l)?,
        born: Persist::load(l)?,
    })
}

impl<H: StoreHost + 'static> TexMachine<H> {
    /// Save the machine; `false` if it cannot be saved.
    pub fn save_machine(&self, s: &mut Saver) -> bool {
        let Self {
            tex,
            command_line,
            started,
            halted,
            fresh,
            at_boundary,
            file_at,
            steps,
            entry,
            snap_at,
            rest_version,
            cut_at,
            region_lines,
            opened,
            glyphs,
            replay_exit,
            region_written,
            region_written_reads,
            skips,
            thaw_from,
            written_delta,
            objs,
            num_delta,
            dests_delta,
            font_delta,
            census: _, // (diagnostics: not kept)
        } = self;
        let mut ok = save_tex(tex, s);
        command_line.save(s);
        (*started, *halted, *fresh, *at_boundary).save(s);
        file_at.save(s);
        (*steps, *snap_at, *rest_version, *cut_at).save(s);
        match entry {
            Some(e) => {
                s.enc.u8(1);
                save_snapshot(e, s, &mut ok);
            }
            None => s.enc.u8(0),
        }
        region_lines.save(s);
        opened.save(s);
        glyphs.save(s);
        (*replay_exit, *skips, *thaw_from).save(s);
        region_written.save(s);
        region_written_reads.save(s);
        written_delta.save(s);
        objs.save(s);
        num_delta.save(s);
        dests_delta.save(s);
        font_delta.save(s);
        ok
    }

    /// Load a machine [`TexMachine::save_machine`] saved.
    pub fn load_machine(l: &mut Loader, template: &H) -> Option<Self> {
        Self::load_machine_in(l, &Cx::new(template, None))
    }

    fn load_machine_in(l: &mut Loader, cx: &Cx<H>) -> Option<Self> {
        let tex = load_tex(l, cx.template)?;
        let command_line = Persist::load(l)?;
        let (started, halted, fresh, at_boundary) = Persist::load(l)?;
        let file_at = Persist::load(l)?;
        let (steps, snap_at, rest_version, cut_at) = Persist::load(l)?;
        let entry = match l.dec.u8()? {
            0 => None,
            1 => Some(load_snapshot(l, cx)?),
            _ => return None,
        };
        let region_lines = Persist::load(l)?;
        let opened = Persist::load(l)?;
        let glyphs = Persist::load(l)?;
        let (replay_exit, skips, thaw_from) = Persist::load(l)?;
        Some(Self {
            tex,
            command_line,
            started,
            halted,
            fresh,
            at_boundary,
            file_at,
            steps,
            entry,
            snap_at,
            rest_version,
            cut_at,
            region_lines,
            opened,
            glyphs,
            replay_exit,
            region_written: Persist::load(l)?,
            region_written_reads: Persist::load(l)?,
            skips,
            thaw_from,
            written_delta: Persist::load(l)?,
            objs: Persist::load(l)?,
            num_delta: Persist::load(l)?,
            dests_delta: Persist::load(l)?,
            font_delta: Persist::load(l)?,
            census: super::Census::default(),
        })
    }
}

/// A run of consecutive regions saved as one blob: a fingerprint of the
/// regions (keys, boundaries, guards and the versions of their writes),
/// and the blob, so that a later save of the same regions refers to it
/// again.
#[derive(Clone, Copy, Debug)]
pub struct SavedChunk {
    pub fingerprint: u128,
    pub hash: u128,
}

/// What identifies a run of regions: equal runs are the same regions,
/// recorded from the same state (their versions are content hashes).
fn fingerprint<H: StoreHost>(run: &[(u64, &Trace<TexMachine<H>>)]) -> u128 {
    use core::hash::Hash;
    let mut h = partex_engine::stablehash::StableHasher::new();
    for (k, t) in run {
        (k, t.entry, t.exit, t.cost, t.allocs, t.born).hash(&mut h);
        t.guards.len().hash(&mut h);
        for (c, v) in &t.guards {
            (c, v.0).hash(&mut h);
        }
        t.writes.len().hash(&mut h);
        for (c, v, ver) in &t.writes {
            (c, v.version, ver.0).hash(&mut h);
        }
        (t.holes.len(), t.effects.len()).hash(&mut h);
        for c in &t.holes {
            c.hash(&mut h);
        }
    }
    h.finish128()
}

/// Where a run of regions ends: after a key whose hash says so (about
/// one in eight; content-defined, so that a region added or merged moves
/// only the runs around it), or at 32 regions.
fn ends_chunk(k: u64, len: usize) -> bool {
    len >= 32 || (k.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 61) == 0
}

/// The address tags the parts of a build are shared under (with an
/// address inside the build, alive while it is saved).
const FINAL_PART: usize = usize::MAX - 1;
const CHUNK_PART: usize = usize::MAX - 2;

/// Save a build: its starting state and its digest; its final state and
/// digest, and its regions in runs, each shared (in a [`Saver::merkle`],
/// blobs of their own, loaded apart: [`load_parts`]); its rebuilds'
/// bookkeeping. `reuse`: the blob a run of this fingerprint was saved as
/// before (then referred to, not written again). `None` if the build
/// cannot be saved; else its runs.
pub fn save_build<H: StoreHost + 'static>(
    b: &Build<TexMachine<H>>,
    s: &mut Saver,
    reuse: &dyn Fn(u128) -> Option<u128>,
) -> Option<Vec<SavedChunk>> {
    use partex_incr::Machine as _;
    let (initial, fin, seq, generation, changed) = b.parts();
    let mut ok = initial.save_machine(s);
    initial.digest().0.save(s);
    s.share(
        core::ptr::from_ref(fin) as usize,
        FINAL_PART,
        "final",
        || partex_engine::persist::Pin::None,
        |s| {
            ok &= fin.save_machine(s);
            fin.digest().0.save(s);
        },
    );
    let mut runs: Vec<&RunOf<'_, H>> = Vec::new();
    let mut start = 0;
    for (i, (k, _)) in seq.iter().enumerate() {
        if ends_chunk(*k, i + 1 - start) || i + 1 == seq.len() {
            runs.push(&seq[start..=i]);
            start = i + 1;
        }
    }
    runs.len().save(s);
    let mut out = Vec::with_capacity(runs.len());
    for run in runs {
        let fp = fingerprint(run);
        let hash = if let Some(h) = reuse(fp).filter(|&h| s.blob_ref(h)) {
            h
        } else {
            let addr = core::ptr::from_ref(run[0].1) as usize;
            s.share(
                addr,
                CHUNK_PART,
                "regions",
                || partex_engine::persist::Pin::None,
                |s| {
                    run.len().save(s);
                    for (k, t) in run {
                        k.save(s);
                        save_trace(t, s, &mut ok);
                    }
                },
            );
            s.merkle_hash_of(addr, CHUNK_PART).unwrap_or(0)
        };
        out.push(SavedChunk {
            fingerprint: fp,
            hash,
        });
    }
    generation.save(s);
    changed.save(s);
    ok.then_some(out)
}

/// Regions with their keys, in order.
pub type Regions<H> = Vec<(u64, Trace<TexMachine<H>>)>;

/// A run of regions saved as one part.
type RunOf<'a, H> = [(u64, &'a Trace<TexMachine<H>>)];

/// A part of a saved build: here, or a blob of the store to load it
/// from (with [`load_final`] or [`load_chunk`], on any thread).
pub enum Part<T> {
    Here(T),
    Blob(u128),
}

/// A saved build in parts, the starting state loaded and checked.
pub struct Parts<H: StoreHost> {
    pub initial: TexMachine<H>,
    pub fin: Part<TexMachine<H>>,
    pub chunks: Vec<Part<Regions<H>>>,
    pub generation: u32,
    pub changed: BTreeMap<MCell, u32>,
}

const BAD: &str = "malformed";

/// Read a build [`save_build`] saved, as far as its parts in blobs.
pub fn load_parts<H: StoreHost + 'static>(
    l: &mut Loader,
    cx: &Cx<H>,
) -> Result<Parts<H>, &'static str> {
    use partex_incr::Machine as _;
    let initial = TexMachine::load_machine_in(l, cx).ok_or("the starting state does not load")?;
    if initial.digest().0 != u128::load(l).ok_or(BAD)? {
        return Err("the starting state differs from its digest");
    }
    let fin = if let Some(h) = l.take_blob_ref() {
        Part::Blob(h)
    } else {
        let (f, d) = l
            .share_local(|l| load_final(l, cx).ok())
            .ok_or("the final state does not load")?;
        check_digest(&f, d)?;
        Part::Here(f)
    };
    let n = usize::load(l).ok_or(BAD)?;
    let mut chunks = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        chunks.push(match l.take_blob_ref() {
            Some(h) => Part::Blob(h),
            None => Part::Here(
                l.share_local(|l| load_chunk(l, cx))
                    .ok_or("a region does not load")?,
            ),
        });
    }
    Ok(Parts {
        initial,
        fin,
        chunks,
        generation: u32::load(l).ok_or(BAD)?,
        changed: Persist::load(l).ok_or(BAD)?,
    })
}

/// The final state, from its part's bytes, and the digest saved with it
/// (for [`check_digest`]).
pub fn load_final<H: StoreHost + 'static>(
    l: &mut Loader,
    cx: &Cx<H>,
) -> Result<(TexMachine<H>, u128), &'static str> {
    let fin = TexMachine::load_machine_in(l, cx).ok_or("the final state does not load")?;
    Ok((fin, u128::load(l).ok_or(BAD)?))
}

/// Check a state loaded against the digest saved with it.
pub fn check_digest<H: StoreHost + 'static>(
    m: &TexMachine<H>,
    digest: u128,
) -> Result<(), &'static str> {
    use partex_incr::Machine as _;
    if m.digest().0 == digest {
        Ok(())
    } else {
        Err("the final state differs from its digest")
    }
}

/// The [`SavedChunk`] of a run of regions loaded from blob `hash`.
#[must_use]
pub fn chunk_of<H: StoreHost>(run: &[(u64, Trace<TexMachine<H>>)], hash: u128) -> SavedChunk {
    let refs: Vec<(u64, &Trace<TexMachine<H>>)> = run.iter().map(|(k, t)| (*k, t)).collect();
    SavedChunk {
        fingerprint: fingerprint(&refs),
        hash,
    }
}

/// A run of regions, from its part's bytes.
pub fn load_chunk<H: StoreHost + 'static>(
    l: &mut Loader,
    cx: &Cx<H>,
) -> Option<Vec<(u64, Trace<TexMachine<H>>)>> {
    let n = usize::load(l)?;
    let mut run = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let k = u64::load(l)?;
        run.push((k, load_trace(l, cx)?));
    }
    Some(run)
}

/// The build of `parts` whose parts are all here (`index`: its indexes,
/// made apart, if they were).
pub fn assemble<H: StoreHost + 'static>(
    initial: TexMachine<H>,
    fin: TexMachine<H>,
    seq: Vec<(u64, Trace<TexMachine<H>>)>,
    generation: u32,
    changed: BTreeMap<MCell, u32>,
    index: Option<partex_incr::Index<TexMachine<H>>>,
) -> Result<Build<TexMachine<H>>, &'static str> {
    if !seq.windows(2).all(|w| w[0].0 < w[1].0) {
        return Err("regions out of order");
    }
    Ok(match index {
        Some(ix) => Build::from_parts_indexed(initial, fin, seq, generation, changed, ix),
        None => Build::from_parts(initial, fin, seq, generation, changed),
    })
}

/// [`assemble`] with the final state still to come (`make` gives it
/// when first needed) and the indexes made.
pub fn assemble_later<H: StoreHost + 'static>(
    initial: TexMachine<H>,
    make: partex_incr::MakeLater<TexMachine<H>>,
    seq: Vec<(u64, Trace<TexMachine<H>>)>,
    generation: u32,
    changed: BTreeMap<MCell, u32>,
    index: Option<partex_incr::Index<TexMachine<H>>>,
) -> Result<Build<TexMachine<H>>, &'static str> {
    if !seq.windows(2).all(|w| w[0].0 < w[1].0) {
        return Err("regions out of order");
    }
    let index = index.ok_or("no index")?;
    Ok(Build::from_parts_later(
        initial, make, seq, generation, changed, index,
    ))
}

/// Load a build [`save_build`] saved, its parts in blobs by `fetch`, on
/// this thread: `Err` if anything is malformed or a state's digest is not
/// the one saved (a state the engine does not save whole: never a wrong
/// build, a cold one).
pub fn load_build<H: StoreHost + 'static>(
    l: &mut Loader,
    cx: &Cx<H>,
    fetch: partex_engine::persist::Fetch<'_>,
) -> Result<Build<TexMachine<H>>, &'static str> {
    let p = load_parts(l, cx)?;
    let blob = |h: u128| fetch(h).ok_or("a part is missing");
    let fin = match p.fin {
        Part::Here(f) => f,
        Part::Blob(h) => {
            let bytes = blob(h)?;
            let (f, d) = load_final(
                &mut Loader::merkle(&bytes, fetch, partex_engine::persist::Loaded::new()),
                cx,
            )?;
            check_digest(&f, d)?;
            f
        }
    };
    let mut seq = Vec::new();
    for c in p.chunks {
        seq.extend(match c {
            Part::Here(run) => run,
            Part::Blob(h) => {
                let bytes = blob(h)?;
                load_chunk(
                    &mut Loader::merkle(&bytes, fetch, partex_engine::persist::Loaded::new()),
                    cx,
                )
                .ok_or("a region does not load")?
            }
        });
    }
    assemble(p.initial, fin, seq, p.generation, p.changed, None)
}

/// How many guards and writes of each kind a build holds (for
/// `PARTEX_STORE_DEBUG`).
pub fn census<H: StoreHost + 'static>(b: &Build<TexMachine<H>>) -> BTreeMap<&'static str, usize> {
    let mut m = BTreeMap::new();
    for t in b.traces() {
        *m.entry("guards").or_default() += t.guards.len();
        for (c, _) in &t.guards {
            let k = match c {
                MCell::Rest => "g.rest",
                MCell::Glyphs => "g.glyphs",
                MCell::Classes => "g.classes",
                MCell::File(_) => "g.file",
                MCell::Line(..) => "g.line",
                MCell::Positions => "g.positions",
                MCell::Page => "g.page",
                MCell::PdfLast(_) => "g.pdf last",
                MCell::PdfWord(_) => "g.pdf word",
                MCell::FinalNum(_) => "g.final num",
                MCell::OfFinal(_) => "g.of final",
                MCell::NumState => "g.num state",
                MCell::Sealed(..) => "g.sealed",
                MCell::Written(_) => "g.written",
                MCell::Eqtb(p) if *p >= crate::xregs::EXT_BASE => "g.xreg",
                MCell::Eqtb(_) => "g.eqtb",
                MCell::Link(_) => "g.link",
                MCell::Name(_) => "g.name",
                MCell::Font(_) => "g.font",
                MCell::FontName(_) => "g.font name",
                MCell::FontExpand(..) => "g.font expand",
                MCell::FontOrder => "g.font order",
                MCell::Obj(_) => "g.obj",
                MCell::Tree(..) => "g.tree",
                MCell::Dests => "g.dests",
                MCell::Numbering => "g.numbering",
            };
            *m.entry(k).or_default() += 1;
        }
        for (_, v, _) in &t.writes {
            let k = match &v.v {
                V::Version => "version",
                V::Word(_) => "word",
                V::Lazy(..) => "lazy",
                V::Rest(_) => "rest",
                V::File(_) => "file",
                V::Line => "line",
                V::Positions(_) => "positions",
                V::Page(_) => "page",
                V::Glyphs(_) => "glyphs",
                V::Sealed(_) => "sealed",
                V::Written(_) => "written",
                V::Obj(_) => "obj",
                V::Tree(_) => "tree",
                V::Dests(_) => "dests",
                V::Numbering(_) => "numbering",
                V::Int(_) => "int",
                V::Font(_) => "font",
                V::FontOrder(_) => "font order",
            };
            *m.entry(k).or_default() += 1;
        }
    }
    m
}

/// Check every snapshot a build holds against the version its value
/// carries (`PARTEX_STORE_CHECK=1`): the count of snapshots that differ.
pub fn check_snapshots<H: StoreHost + 'static>(b: &Build<TexMachine<H>>) -> (usize, usize) {
    let (mut n, mut bad) = (0, 0);
    for t in b.traces() {
        for (c, v, _) in &t.writes {
            if let (MCell::Rest, V::Rest(s)) = (c, &v.v) {
                n += 1;
                let tex = s.engine();
                let host = tex.host.clone();
                let served = |name: &[u8]| TexMachine::<H>::serves(&host, name);
                let state = tex.rest_hash_served(&served);
                let mut h = partex_engine::stablehash::StableHasher::new();
                core::hash::Hasher::write_u128(&mut h, state);
                core::hash::Hasher::write_u128(&mut h, tex.host.digest());
                core::hash::Hash::hash(&(s.started, s.halted), &mut h);
                if h.finish128() != v.version {
                    bad += 1;
                }
            }
        }
    }
    (n, bad)
}

#[cfg(test)]
mod tests {
    //! A build holding every kind of cell and value survives the store:
    //! saved, loaded and saved again, it encodes to the same bytes, plain
    //! and as blobs. A new kind of cell or value fails to compile here
    //! (the matches below) until it has a sample, and so a round trip.

    use alloc::collections::{BTreeMap, BTreeSet};
    use alloc::string::String;
    use alloc::sync::Arc;
    use alloc::vec;
    use alloc::vec::Vec;

    use partex_engine::persist::{Loaded, Loader, Persist, Saver};
    use partex_incr::{Build, Trace, Version};

    use super::super::{
        CellValue, LazyBody, Lists, MCell, MValue, Named, SnapBody, Snapshot, TexMachine, V,
    };
    use super::{Cx, StoreHost, load_build, save_build};
    use crate::host::{DateTime, FileKind, Host, OpenedFile, WriteId};
    use crate::machine::{CellHost, CellTracker, FileLines};
    use crate::params::Params;
    use crate::pdf::objtab::{Aux, Entry, Id};
    use crate::tex::Tex;

    /// A host with a little state of its own.
    #[derive(Clone, Default)]
    struct H0 {
        own: u32,
    }

    impl Host for H0 {
        fn read_file(&mut self, _: &[u8], _: FileKind) -> Option<OpenedFile> {
            None
        }
        fn open_write(&mut self, name: &[u8], _: FileKind) -> Option<(WriteId, Vec<u8>)> {
            Some((WriteId(1), name.to_vec()))
        }
        fn write(&mut self, _: WriteId, _: &[u8]) {}
        fn close(&mut self, _: WriteId) {}
        fn term_write(&mut self, _: &[u8]) {}
        fn term_read_line(&mut self) -> Option<Vec<u8>> {
            None
        }
        fn now(&self) -> DateTime {
            DateTime {
                year: 1776,
                month: 7,
                day: 4,
                minutes: 720,
            }
        }
    }

    impl CellHost for H0 {
        fn take_reads(&mut self) -> Vec<Vec<u8>> {
            Vec::new()
        }
        fn file(&self, _: &[u8]) -> Option<Arc<[u8]>> {
            None
        }
        fn set_file(&mut self, _: &[u8], _: Option<Arc<[u8]>>) {}
        fn lines(&self, _: &[u8]) -> Option<FileLines> {
            None
        }
        fn digest(&self) -> u128 {
            u128::from(self.own)
        }
        fn written(&self, _: u32) -> Option<Arc<Vec<u8>>> {
            None
        }
        fn written_ids(&self) -> Vec<u32> {
            Vec::new()
        }
        fn append_written(&mut self, _: u32, _: &[u8]) {}
        fn reads_back(&self, _: &[u8]) -> bool {
            false
        }
        fn take_written(&mut self) -> (BTreeMap<u32, Vec<u8>>, Vec<u32>) {
            (BTreeMap::new(), Vec::new())
        }
        fn clone_state_from(&self, snapshot: &Self) -> Self {
            snapshot.clone()
        }
    }

    impl StoreHost for H0 {
        fn save_own(&self, s: &mut Saver) {
            self.own.save(s);
        }
        fn load_own(&self, l: &mut Loader) -> Option<Self> {
            Some(Self { own: u32::load(l)? })
        }
    }

    fn engine(own: u32) -> Tex<H0, CellTracker> {
        let mut t = Tex::new(H0 { own }, CellTracker::default(), Params::trip());
        t.init_charset();
        t.init_output();
        assert!(t.get_strings_started().unwrap());
        t.init_eqtb();
        t.init_xeq_level();
        t.init_hash();
        t.init_nest();
        t
    }

    /// One cell of every kind.
    fn cells() -> Vec<MCell> {
        let name: Arc<[u8]> = Arc::from(&b"a.tex"[..]);
        let v = vec![
            MCell::Rest,
            MCell::Glyphs,
            MCell::Classes,
            MCell::File(name.clone()),
            MCell::Line(name.clone(), 3),
            MCell::Positions,
            MCell::Page,
            MCell::PdfLast(6),
            MCell::PdfWord(4),
            MCell::FinalNum(7),
            MCell::OfFinal(8),
            MCell::NumState,
            MCell::Sealed(1, 2),
            MCell::Written(4),
            MCell::Eqtb(5),
            MCell::Eqtb(crate::xregs::EXT_BASE + 5),
            MCell::Link(600),
            MCell::Name(Arc::from(&b"name"[..])),
            MCell::Font(3),
            MCell::FontName(Arc::from(&b"cmr10"[..])),
            MCell::FontExpand(3, 20),
            MCell::FontOrder,
            MCell::Obj(6),
            MCell::Tree(1, Arc::from(&b"\x01name"[..])),
            MCell::Dests,
            MCell::Numbering,
        ];
        let mut kinds = BTreeSet::new();
        for c in &v {
            kinds.insert(match c {
                MCell::Rest => 0,
                MCell::Glyphs => 1,
                MCell::Classes => 2,
                MCell::File(_) => 3,
                MCell::Line(..) => 4,
                MCell::Positions => 19,
                MCell::Page => 20,
                MCell::PdfLast(_) => 21,
                MCell::PdfWord(_) => 25,
                MCell::FinalNum(_) => 22,
                MCell::OfFinal(_) => 23,
                MCell::NumState => 24,
                MCell::Sealed(..) => 5,
                MCell::Written(_) => 6,
                // (a register above 255 is an eqtb cell past eqtb)
                MCell::Eqtb(p) if *p >= crate::xregs::EXT_BASE => 12,
                MCell::Eqtb(_) => 7,
                MCell::Obj(_) => 8,
                MCell::Tree(..) => 9,
                MCell::Dests => 10,
                MCell::Numbering => 11,
                MCell::Link(_) => 13,
                MCell::Name(_) => 14,
                MCell::Font(_) => 15,
                MCell::FontName(_) => 16,
                MCell::FontExpand(..) => 17,
                MCell::FontOrder => 18,
            });
        }
        assert_eq!(kinds.len(), 26, "a sample of every kind of cell");
        v
    }

    /// One value of every kind (two sharing a snapshot).
    fn values(snap: &Arc<Snapshot<H0>>) -> Vec<MValue<H0>> {
        let word = CellValue {
            word: crate::mem::MemoryWord::default(),
            level: Some(1),
            named: Named::Nothing,
            name: None,
        };
        let entry = Entry {
            info: Id::Name(Arc::from(&b"x"[..])),
            link: 3,
            offset: -2,
            os_idx: 0,
            aux: Aux::Int(9),
        };
        let list = CellValue {
            word: crate::mem::MemoryWord::default(),
            level: None,
            named: Named::List {
                toks: Arc::from(&[1, 2][..]),
                protected: false,
                interned: true,
            },
            name: Some(Arc::from(&b"foo"[..])),
        };
        let v = vec![
            V::Version,
            V::Word(Arc::new(word)),
            V::Word(Arc::new(list)),
            V::Lazy(snap.clone(), 100),
            V::Rest(snap.clone()),
            V::File(Arc::new(Arc::from(&b"contents\n"[..]))),
            V::Line,
            V::Positions(Arc::new(super::super::Positions {
                lines: vec![0, 3, 7],
                files: vec![
                    (1, Arc::from(&b"a.tex"[..]), 7),
                    (258, Arc::from(&b"b.tex"[..]), 2),
                ],
            })),
            V::Page(Arc::new(super::super::PageValue {
                builder: partex_engine::builder::Builder {
                    so_far: [1, 2, 3, 4, 5, 6, 7, 8],
                    insert_penalties: 9,
                    discards: vec![partex_engine::node::Node::Penalty(10)].into(),
                    ..partex_engine::builder::Builder::default()
                },
                list: vec![
                    Arc::from(&[partex_engine::node::Node::Penalty(11)][..]),
                    Arc::from(
                        &[partex_engine::node::Node::Kern {
                            width: 12,
                            subtype: 0,
                            sync: partex_engine::origin::Side(0),
                        }][..],
                    ),
                ],
            })),
            V::Glyphs(Arc::new(vec![[1, 2, 3, 4]])),
            V::Sealed(Arc::new(crate::seal::Sealed::new(
                0.5,
                partex_engine::node::GlueSign::Stretching,
                partex_engine::node::Order::Fil,
                Vec::new(),
            ))),
            V::Written(Arc::new(b"\\relax\n".to_vec())),
            V::Obj(Arc::new(entry)),
            V::Tree(12),
            V::Int(14),
            V::Font(Arc::new(super::super::FontSlot {
                ident: 5,
                retagged: false,
                metrics: Arc::new(partex_engine::font::Font::null()),
                tfm: Arc::from(&b"tfm"[..]),
                name: Arc::from(&b"cmr10"[..]),
                area: Arc::from(&b""[..]),
                hyphen_char: 45,
                skew_char: -1,
                codes: crate::fonts::Codes::default(),
                expand: crate::fonts::Expand::default(),
                pdf: crate::pdf::ship::PdfFont::default(),
                native: None,
                native_dir: 0,
            })),
            V::FontOrder(Arc::new(vec![3, 4])),
            V::Dests(Arc::new(vec![(Arc::from(&b"dest"[..]), 13)])),
            V::Numbering(Arc::new(vec![
                crate::pdf::vnum::NumEvent::Create(5),
                crate::pdf::vnum::NumEvent::Start,
                crate::pdf::vnum::NumEvent::End,
                crate::pdf::vnum::NumEvent::Flush,
            ])),
        ];
        let mut kinds = BTreeSet::new();
        for x in &v {
            kinds.insert(match x {
                V::Version => 0,
                V::Word(_) => 1,
                V::Lazy(..) => 2,
                V::Rest(_) => 3,
                V::File(_) => 4,
                V::Line => 5,
                V::Glyphs(_) => 6,
                V::Sealed(_) => 7,
                V::Written(_) => 8,
                V::Obj(_) => 9,
                V::Tree(_) => 10,
                V::Dests(_) => 11,
                V::Numbering(_) => 12,
                V::Int(_) => 13,
                V::Font(_) => 14,
                V::FontOrder(_) => 15,
                V::Positions(_) => 16,
                V::Page(_) => 17,
            });
        }
        assert_eq!(kinds.len(), 18, "a sample of every kind of value");
        assert!(
            v.len() > kinds.len(),
            "and a value naming a list, as a register above 255's does"
        );
        v.into_iter()
            .enumerate()
            .map(|(i, v)| MValue {
                version: 1000 + i as u128,
                v,
            })
            .collect()
    }

    fn build() -> Build<TexMachine<H0>> {
        let snap = Arc::new(Snapshot::here(SnapBody::new(
            engine(7),
            Lists(vec![vec![Arc::from(Vec::new())]]),
            true,
            false,
        )));
        let cells = cells();
        let values = values(&snap);
        let trace = Trace {
            entry: super::super::Pos {
                key: 1,
                file: 3,
                line: 4,
            },
            exit: super::super::Pos {
                key: 2,
                file: 3,
                line: 5,
            },
            guards: cells.iter().map(|c| (c.clone(), Version(3))).collect(),
            writes: cells
                .iter()
                .cycle()
                .zip(values)
                .enumerate()
                .map(|(i, (c, v))| (c.clone(), v, Version(i as u128)))
                .collect(),
            effects: Vec::new(),
            holes: vec![MCell::Rest],
            allocs: 4,
            cost: 5,
            born: 1,
        };
        let changed = cells.iter().map(|c| (c.clone(), 1)).collect();
        Build::from_parts(
            TexMachine::new(engine(1), b"\\relax"),
            TexMachine::new(engine(2), b"\\relax"),
            vec![(10, trace.clone()), (20, trace)],
            1,
            changed,
        )
    }

    fn encode(b: &Build<TexMachine<H0>>) -> Vec<u8> {
        let mut s = Saver::new();
        assert!(save_build(b, &mut s, &|_| None).is_some());
        s.into_bytes()
    }

    #[test]
    fn every_cell_and_value_round_trips() {
        // (it hashes `Rest` twice: under the same switches)
        let _switches = Switches::take();
        let b = build();
        let template = H0::default();
        let bytes = encode(&b);
        let mut l = Loader::new(&bytes);
        let back = load_build(&mut l, &Cx::new(&template, None), &|_| None).expect("loads");
        assert_eq!(encode(&back), bytes);
    }

    type Blobs = BTreeMap<u128, Vec<u8>>;

    fn merkle(
        b: &Build<TexMachine<H0>>,
        have: &Blobs,
        reuse: &dyn Fn(u128) -> Option<u128>,
    ) -> (Blobs, Vec<u8>, Vec<super::SavedChunk>) {
        let mut s = Saver::merkle(16, have.keys().copied().collect());
        let chunks = save_build(b, &mut s, reuse).expect("saves");
        let (blobs, root) = s.into_merkle();
        (
            blobs.into_iter().map(|(h, b, _)| (h, b)).collect(),
            root,
            chunks,
        )
    }

    /// The census names every kind of guard and value apart (a new kind
    /// must be counted under a name of its own).
    #[test]
    fn the_census_counts_every_kind() {
        let m = super::census(&build());
        let guards = m.keys().filter(|k| k.starts_with("g.")).count();
        assert_eq!(guards, cells().len(), "{m:?}");
        let values = m
            .keys()
            .filter(|k| !k.starts_with("g.") && **k != "guards")
            .count();
        assert_eq!(values, 18, "{m:?}");
    }

    #[test]
    fn every_cell_and_value_round_trips_as_blobs() {
        // (it hashes `Rest` twice: under the same switches)
        let _switches = Switches::take();
        let b = build();
        let template = H0::default();
        let (blobs, root, _) = merkle(&b, &Blobs::new(), &|_| None);
        assert!(!blobs.is_empty());
        let fetch = |h: u128| blobs.get(&h).cloned();
        let mut l = Loader::merkle(&root, &fetch, Loaded::new());
        let back = load_build(&mut l, &Cx::new(&template, None), &fetch).expect("loads");
        drop(l);
        let (blobs2, root2, _) = merkle(&back, &Blobs::new(), &|_| None);
        assert_eq!(root2, root);
        assert_eq!(blobs2, blobs);
    }

    #[test]
    fn values_known_from_the_last_save_are_not_written_again() {
        // (it hashes `Rest` twice: under the same switches)
        let _switches = Switches::take();
        let b = build();
        let mut s = Saver::merkle(16, BTreeSet::new());
        let chunks = save_build(&b, &mut s, &|_| None).expect("saves");
        let known = s.take_known();
        let (blobs, root) = s.into_merkle();
        assert!(!known.is_empty());
        let have: BTreeSet<u128> = blobs.iter().map(|b| b.0).collect();
        let mut s = Saver::merkle_known(16, have, known);
        let reuse = |fp: u128| chunks.iter().find(|c| c.fingerprint == fp).map(|c| c.hash);
        save_build(&b, &mut s, &reuse).expect("saves");
        let (new, root2) = s.into_merkle();
        assert_eq!(root2, root);
        assert_eq!(new.len(), 0);
    }

    /// A snapshot from the store, as [`MakeLazy`](super::MakeLazy) gives.
    struct Loaded0(SnapBody<H0>);

    impl LazyBody<H0> for Loaded0 {
        fn body(&self) -> &SnapBody<H0> {
            &self.0
        }
    }

    #[test]
    fn stored_parts_are_saved_as_references() {
        // (it hashes `Rest` twice: under the same switches)
        let _switches = Switches::take();
        let b = build();
        let template = H0::default();
        let (blobs, root, chunks) = merkle(&b, &Blobs::new(), &|_| None);
        let fetch = |h: u128| blobs.get(&h).cloned();
        let made = core::sync::atomic::AtomicUsize::new(0);
        let make = |h: u128| -> alloc::boxed::Box<dyn LazyBody<H0>> {
            made.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            let bytes = blobs.get(&h).expect("blob");
            let mut l = Loader::merkle(bytes, &fetch, Loaded::new());
            alloc::boxed::Box::new(Loaded0(
                super::load_snapshot_body(&mut l, &template).expect("body"),
            ))
        };
        let mut l = Loader::merkle(&root, &fetch, Loaded::new());
        let back = load_build(&mut l, &Cx::new(&template, Some(&make)), &fetch).expect("loads");
        drop(l);
        // (one snapshot, met in many values, made once)
        assert_eq!(made.load(core::sync::atomic::Ordering::Relaxed), 1);
        // saved again, the runs of regions referred to as they were: the
        // same root, and no blob written anew
        let reuse = |fp: u128| chunks.iter().find(|c| c.fingerprint == fp).map(|c| c.hash);
        let (new, root2, _) = merkle(&back, &blobs, &reuse);
        assert_eq!(root2, root);
        assert_eq!(new.len(), 0);
    }

    /// Bug D (DESIGN §7.12): a token list's input record names the control
    /// sequence a macro was called by (§390), a location, and `Rest`
    /// hashed it as a string. A state set from cells has strings the run it
    /// came from never made (names imported by their characters), so the
    /// same location fell among its strings in one state and past them in
    /// the other: equal states, different `Rest` versions.
    #[test]
    fn a_macro_records_name_is_a_location_not_a_string() {
        // (it hashes `Rest` twice: under the same switches)
        let _switches = Switches::take();
        let with = |extra: usize| {
            let mut t = engine(1);
            t.set_canon_strings(true);
            t.set_name_cells(true);
            // (as if the format's strings ended here)
            t.init_str_ptr = t.str_ptr;
            for k in 0..extra {
                let s = alloc::format!("imported{k}");
                t.intern_str(s.as_bytes());
            }
            t.cur_input = crate::input::InStateRecord {
                list: Some(partex_engine::node::TokenList::shared(&[])),
                state: crate::web::TOKEN_LIST,
                index: 5,
                start: 0,
                loc: 0,
                limit: 0,
                // (between the two states' string counts)
                name: i32::try_from(t.init_str_ptr).unwrap_or(0) + 2,
            };
            t
        };
        let (a, b) = (with(0), with(4));
        assert_ne!(a.str_ptr, b.str_ptr);
        assert_eq!(a.rest_hash(), b.rest_hash());
    }

    /// Bug D: how far the PDF writer's per-font table reaches is not
    /// state; a state set from font cells reaches as far as the fonts it
    /// was given. The glyphs used compare by the fonts that have some.
    #[test]
    fn glyphs_used_do_not_count_empty_fonts() {
        let mut a = engine(1);
        let mut b = engine(1);
        for t in [&mut a, &mut b] {
            t.pdf
                .ship
                .fonts
                .resize(3, crate::pdf::ship::PdfFont::default());
            t.pdf.ship.fonts[2].chars = [1, 0, 0, 4];
        }
        b.pdf
            .ship
            .fonts
            .resize(40, crate::pdf::ship::PdfFont::default());
        assert_eq!(
            TexMachine::<H0>::glyphs_version(&a),
            TexMachine::<H0>::glyphs_version(&b)
        );
    }

    /// With positions as cells, `Rest` has no line numbers of a served
    /// file (the lines read, `line`, the line stack): two states that
    /// differ only in them have one `Rest`, and `MCell::Positions` holds
    /// them; setting it reads the file on from its line.
    #[test]
    fn positions_leave_rest_as_a_cell_of_their_own() {
        use core::sync::atomic::Ordering::Relaxed;
        let _switches = Switches::take();
        let mut m = TexMachine::new(engine(1), b"");
        let at = |m: &mut TexMachine<H0>, k: u32, pos: usize| {
            let t = &mut m.tex;
            t.in_open = 1;
            t.line_stack[1] = 4;
            t.line = i32::try_from(k).unwrap();
            t.input_file[1] = Some(crate::input::AlphaFile {
                data: Arc::from(&b"a\nb\nc\n"[..]),
                pos,
                lines: k,
                name: Arc::from(&b"a.tex"[..]),
                ..crate::input::AlphaFile::default()
            });
        };
        let served = |n: &[u8]| n == b"a.tex";
        let rest = |m: &TexMachine<H0>, on: bool| {
            crate::statehash::POSITION_CELLS.store(on, Relaxed);
            let h = m.tex.rest_hash_served(&served);
            crate::statehash::POSITION_CELLS.store(false, Relaxed);
            h
        };
        at(&mut m, 2, 4);
        let (off2, on2) = (rest(&m, false), rest(&m, true));
        assert_eq!(m.positions().lines, vec![0, 4, 2]);
        at(&mut m, 3, 6);
        assert_ne!(off2, rest(&m, false), "without, `Rest` has the lines");
        assert_eq!(on2, rest(&m, true), "with, it has none");
        // (setting the positions: the counters, and the file read on from
        // the line)
        at(&mut m, 1, 2);
        m.set_positions(&super::super::Positions {
            lines: vec![0, 4, 3],
            files: vec![(1, Arc::from(&b"a.tex"[..]), 3)],
        });
        let f = m.tex.input_file[1].as_ref().unwrap();
        assert_eq!((m.tex.line, f.lines, f.pos), (3, 3, 6));
    }

    /// Bug D: a soft read's restore counted as no change although the
    /// value restored was not the entry's. `{\setbox0.. \global\setbox0..
    /// \setbox0..}` saves box 0 twice at one level: the entry's value, then
    /// (after the global assignment) the global one, which the group's end
    /// restores; the entry's never comes back, so the location was written.
    #[test]
    fn a_second_save_at_one_level_is_a_write() {
        use crate::track::{Cell, Tracker};
        let mut t = CellTracker::default();
        t.reserve(64, 0, 0);
        let c = Cell::Eqtb(40);
        // (the local assignment: a soft read, its save, the write)
        t.soft_read(c, 2);
        t.saved(c, 2);
        t.write(c);
        // (the global assignment, then a local one saving its value)
        t.write(c);
        t.saved(c, 2);
        t.write(c);
        // (the group's end restores the global value)
        t.restored(c, 2);
        t.group_end(2);
        assert!(
            t.written().contains(&super::super::Tracked::Eqtb(40)),
            "the location was written"
        );
        // (with one save, the entry's value restored: untouched)
        let mut u = CellTracker::default();
        u.reserve(64, 0, 0);
        u.soft_read(c, 2);
        u.saved(c, 2);
        u.write(c);
        u.restored(c, 2);
        u.group_end(2);
        assert_eq!(u.written(), Vec::new());
    }

    /// A tracker reset by what it logged (`CellTracker::reset`, and
    /// `adopt` on a restore) is a fresh one: over random runs of reads,
    /// writes, soft reads, saves, restores and group ends, a tracker that
    /// ran other runs first reports what a fresh one does, with every
    /// slot's state and every log the same, and the same room.
    #[test]
    fn a_reset_tracker_is_a_fresh_one() {
        use crate::track::{Cell, Tracker};
        use crate::web::HASH_BASE;
        use crate::xregs::EXT_BASE;
        use core::sync::atomic::Ordering::Relaxed;
        // (xorshift: a run is its seed)
        fn run(t: &CellTracker, seed: &mut u64, ops: usize) {
            let mut n = |m: u64| {
                *seed ^= *seed << 13;
                *seed ^= *seed >> 7;
                *seed ^= *seed << 17;
                i32::try_from(*seed % m).unwrap_or(0)
            };
            for _ in 0..ops {
                let c = match n(5) {
                    0 | 1 => Cell::Eqtb(1 + n(63)),
                    2 => Cell::Eqtb(EXT_BASE + n(8)),
                    3 => Cell::HashNext(HASH_BASE + n(16)),
                    _ => Cell::Font(n(4)),
                };
                let level = 1 + n(4);
                match n(6) {
                    0 => t.read(c),
                    1 => t.write(c),
                    2 => t.soft_read(c, level),
                    3 => t.saved(c, level),
                    4 => t.restored(c, level),
                    _ => t.group_end(level),
                }
            }
        }
        let seen = |t: &CellTracker| {
            let states: Vec<(usize, u8)> = t
                .state
                .iter()
                .map(|s| s.load(Relaxed))
                .enumerate()
                .filter(|&(_, s)| s != 0)
                .collect();
            (
                (t.read_locs(), t.written(), t.soft_pending()),
                (t.reads.to_vec(), t.writes.to_vec(), t.softs.to_vec()),
                (states, t.overflow.get(), t.state.len()),
                (t.reads.room(), t.writes.room(), t.softs.room()),
            )
        };
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        for round in 0..48 {
            // (with and without names and fonts as cells: another layout)
            let (links, fonts) = if round % 2 == 0 { (16, 4) } else { (0, 0) };
            let mut kept = CellTracker::default();
            kept.reserve(64, links, fonts);
            run(&kept, &mut seed, 40);
            kept.clear();
            kept.reserve(64, 16 - links, 4 - fonts);
            run(&kept, &mut seed, 40);
            if round % 3 == 0 {
                // (a restore: the new engine's fresh tracker takes them)
                let mut new = CellTracker::default();
                new.adopt(&mut kept);
                kept = new;
            } else {
                kept.reset();
            }
            // (every slot 0 again, the tables kept)
            assert!(!kept.state.is_empty());
            assert!(kept.state.iter().all(|s| s.load(Relaxed) == 0));
            let mut fresh = CellTracker::default();
            kept.reserve(64, links, fonts);
            fresh.reserve(64, links, fonts);
            assert!(seen(&kept) == seen(&fresh), "round {round}: reset");
            for _ in 0..3 {
                let (mut a, mut b) = (seed, seed);
                run(&kept, &mut a, 30);
                run(&fresh, &mut b, 30);
                seed = a;
                assert!(seen(&kept) == seen(&fresh), "round {round}");
                kept.clear();
                fresh.clear();
            }
        }
    }

    /// A lookup of a name reads where that name is, or where it would be,
    /// and not the chain it hashes into: a name another run made in the
    /// same chain (a `\label`'s `\r@key`) changes no lookup of another
    /// name, while a lookup that found a name missing reads the place it
    /// is made at later.
    #[test]
    fn a_lookup_reads_where_its_name_would_be_not_the_chain() {
        let mut p = Params::trip();
        p.hash_extra = 4096;
        let mut t = Tex::new(H0 { own: 1 }, CellTracker::default(), p);
        t.init_charset();
        t.init_output();
        assert!(t.get_strings_started().unwrap());
        t.init_eqtb();
        t.init_xeq_level();
        t.init_hash();
        t.init_nest();
        t.no_new_control_sequence = false;
        t.set_name_cells(true);
        t.set_cs_by_name(true);
        t.set_probe_names(true);
        // (names of one hash code, §261)
        let code = |b: &[u8]| {
            let mut h = i32::from(b[0]);
            for &c in &b[1..] {
                h = (h + h + i32::from(c)) % crate::web::HASH_PRIME;
            }
            h
        };
        let mut same: BTreeMap<i32, Vec<Vec<u8>>> = BTreeMap::new();
        let names = loop {
            let k = same.values().map(Vec::len).sum::<usize>();
            let n = alloc::format!("n{k}").into_bytes();
            let v = same.entry(code(&n)).or_default();
            v.push(n);
            if v.len() == 4 {
                break v.clone();
            }
        };
        let (n, links) = (t.eqtb.len(), t.hash.len());
        t.tracker.reserve(n, links, 0);
        let look = |t: &mut Tex<H0, CellTracker>, name: &[u8], new: bool| {
            t.no_new_control_sequence = !new;
            t.cs_cache = crate::hash::CsCache::default();
            for (i, &b) in name.iter().enumerate() {
                t.buffer[i] = u32::from(b);
            }
            t.tracker.clear();
            let p = t.id_lookup(0, name.len()).unwrap();
            let (r, w) = (t.tracker.read_locs(), t.tracker.written());
            (p, r, w)
        };
        let (a, b, c, d) = (&names[0], &names[1], &names[2], &names[3]);
        let (pa, _, _) = look(&mut t, a, true);
        let (pb, _, _) = look(&mut t, b, true);
        assert_ne!(pa, pb);
        // (c missing: its lookup reads the place c then goes to)
        let (none, missing, _) = look(&mut t, c, false);
        assert_eq!(none, crate::eqtb::UNDEFINED_CONTROL_SEQUENCE);
        let (pc, _, made) = look(&mut t, c, true);
        assert!(
            made.iter().any(|w| missing.contains(w)),
            "{missing:?} {made:?}"
        );
        // (b looked up again reads nothing that making d, in the same
        // chain, writes)
        let (pb2, found, _) = look(&mut t, b, false);
        assert_eq!(pb2, pb);
        let (pd, _, made) = look(&mut t, d, true);
        assert!(![pa, pb, pc].contains(&pd));
        assert!(
            !made.iter().any(|w| found.contains(w)),
            "{found:?} {made:?}"
        );
        assert!(
            made.iter()
                .all(|w| matches!(w, super::super::Tracked::Eqtb(_))),
            "no links: {made:?}"
        );
    }

    /// A recorder that keeps nothing: the machine's steps alone.
    struct Nothing;

    impl partex_incr::Recorder<TexMachine<H0>> for Nothing {
        fn read(&mut self, _: &MCell, _: Option<&MValue<H0>>) {}
        fn write(&mut self, _: &MCell, _: &MValue<H0>) {}
        fn effect(&mut self, _: crate::effects::Effect) {}
        fn force(&mut self, _: partex_incr::Hole) -> partex_incr::Forced<MValue<H0>> {
            unreachable!("no holes")
        }
    }

    /// Candidate levels (DESIGN §7.16.1) on a tiny job read from the
    /// command line (INITEX, `\nullfont`, where a letter returns to
    /// `big_switch`, §1036, after the backed-up list that held it, which
    /// the eager pop removes): the boundary between two paragraphs is an
    /// outer clean point, in a group too; a paragraph's first candidate is
    /// its start, indented or not, and the others are not clean; a
    /// paragraph `\everypar` ends at once has no start; the paragraph
    /// goes on after a display (§1200) with a start again (the display
    /// has no math fonts: an error, and an empty formula).
    #[test]
    fn clean_points_are_level_3() {
        let params = Params {
            ini: true,
            interaction: Some(crate::error::BATCH_MODE),
            ..Params::trip()
        };
        let tex = Tex::new(H0::default(), CellTracker::default(), params);
        let job =
            b"\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\$=3 \\relax a b\\par c\\par\\noindent\
                    \\relax d\\par{\\everypar{\\par}\\noindent\\relax}e $$ $$ g\\end";
        let mut m = TexMachine::new(tex, job);
        // (the rest of the line at each candidate, its level, and which
        // clean point it is: 0 outer, 1 a paragraph's start)
        let mut seen = Vec::new();
        loop {
            let before = m.census().clean;
            match partex_incr::Machine::step(&mut m, &mut Nothing) {
                partex_incr::Step::Halt => break,
                partex_incr::Step::Candidate(level) => {
                    let t = &m.tex;
                    let from = usize::try_from(t.cur_input.loc).unwrap();
                    let to = usize::try_from(t.cur_input.limit).unwrap();
                    let rest: Vec<u8> = (from..to)
                        .map(|k| u8::try_from(t.buffer[k]).unwrap())
                        .collect();
                    let after = m.census().clean;
                    let kind = (0..2).find(|&k| after[k] != before[k]);
                    seen.push((String::from_utf8(rest).unwrap(), level, kind));
                }
                _ => {}
            }
        }
        let at = |s: &str| {
            let l: Vec<(u8, Option<usize>)> = seen
                .iter()
                .filter(|(r, _, _)| r.starts_with(s))
                .map(|&(_, l, k)| (l, k))
                .collect();
            assert_eq!(l.len(), 1, "one candidate before `{s}`: {seen:?}");
            l[0]
        };
        assert_eq!(at(" c\\par"), (3, Some(0)), "between two paragraphs");
        assert_eq!(at(" b\\par"), (3, Some(1)), "an indented paragraph's start");
        assert_eq!(at("b\\par"), (1, None), "inside a paragraph");
        assert_eq!(at("\\relax d"), (3, Some(1)), "after `\\noindent`");
        assert_eq!(at(" d\\par"), (1, None), "inside, the list empty");
        assert_eq!(at("\\relax}e"), (3, Some(0)), "no start, in a group");
        assert_eq!(at("}e"), (3, Some(0)), "in a group");
        assert_eq!(at(" $$ $$ g"), (3, Some(1)), "`e`'s paragraph");
        assert_eq!(at("g\\end"), (3, Some(1)), "after a display");
        assert_eq!(at("\\end"), (1, None), "inside, after a display");
        assert_eq!(m.census().seen.iter().sum::<u64>(), seen.len() as u64);
    }

    /// Held by a test that sets the process's switches, or hashes `Rest`
    /// under them more than once and compares (the tests run in
    /// parallel: a switch flipped between two hashes of one test makes
    /// them differ, as `values_known_from_the_last_save_are_not_written_again`
    /// did once in the gate of 1efe489).
    struct Switches;

    static SWITCHES: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

    impl Switches {
        fn take() -> Self {
            use core::sync::atomic::Ordering::Acquire;
            while SWITCHES
                .compare_exchange(false, true, Acquire, core::sync::atomic::Ordering::Relaxed)
                .is_err()
            {
                core::hint::spin_loop();
            }
            Self
        }
    }

    impl Drop for Switches {
        fn drop(&mut self) {
            SWITCHES.store(false, core::sync::atomic::Ordering::Release);
        }
    }

    /// With the page as a cell (the default), `Rest` has no page builder
    /// state: two states that differ only in it have one `Rest`, and
    /// `MCell::Page` tells them apart; setting it back gives the value
    /// back, and a snapshot's value is the running engine's.
    #[test]
    fn the_page_leaves_rest_as_a_cell_of_its_own() {
        use partex_engine::node::Node;
        use partex_incr::Machine;
        let _switches = Switches::take();
        let mut m = TexMachine::new(engine(1), b"");
        let served = |_: &[u8]| true;
        let rest = m.tex.rest_hash_served(&served);
        let before = m.get(&MCell::Page).expect("a value");
        m.tex.page.list.push(Node::Penalty(5));
        m.tex.page.insert_penalties = 3;
        assert_eq!(rest, m.tex.rest_hash_served(&served), "not in `Rest`");
        let after = m.get(&MCell::Page).expect("a value");
        assert_ne!(before, after, "in `Page`");
        m.take_snapshot();
        assert_eq!(m.get(&MCell::Page), Some(after.clone()), "a snapshot's");
        m.set(&MCell::Page, Some(before.clone()));
        assert_eq!(m.get(&MCell::Page), Some(before));
        assert!(m.tex.page.list.is_empty() && m.tex.page.insert_penalties == 0);
        m.set(&MCell::Page, Some(after.clone()));
        assert_eq!(m.get(&MCell::Page), Some(after));
    }

    /// The tracker's page state: a write reads first, a read after a
    /// write is not a read, and a region's end forgets both.
    #[test]
    fn the_tracker_sees_the_page_read_and_written() {
        use crate::track::Tracker;
        let t = CellTracker::default();
        assert_eq!(t.page_touched(), (false, false));
        t.page_access(false);
        assert_eq!(t.page_touched(), (true, false));
        t.page_access(true);
        assert_eq!(t.page_touched(), (true, true));
        t.clear();
        t.page_access(true);
        t.page_access(false);
        assert_eq!(t.page_touched(), (true, true));
        t.clear();
        assert_eq!(t.page_touched(), (false, false));
    }

    /// The `\pdflast…` values as cells: a write is not a read, a read
    /// after a write is not one either, and a value set comes back; with
    /// them as cells `Rest` does not see them.
    #[test]
    fn the_pdf_last_values_are_cells_of_their_own() {
        use crate::pdf::PdfLast;
        use crate::track::Tracker;
        use partex_incr::Machine;
        let t = CellTracker::default();
        t.pdf_last_access(PdfLast::Link as u8, false);
        t.pdf_last_access(PdfLast::Obj as u8, true);
        t.pdf_last_access(PdfLast::Obj as u8, false);
        assert_eq!(t.pdf_last_touched(), (1 << 6, 1 << 0));
        t.clear();
        assert_eq!(t.pdf_last_touched(), (0, 0));
        let _switches = Switches::take();
        let mut m = TexMachine::new(engine(1), b"");
        let served = |_: &[u8]| true;
        let rest = m.tex.rest_hash_served(&served);
        let before = m.get(&MCell::PdfLast(6)).expect("a value");
        m.tex.pdf.last_link = 17;
        assert_eq!(rest, m.tex.rest_hash_served(&served), "not in `Rest`");
        let after = m.get(&MCell::PdfLast(6)).expect("a value");
        assert_ne!(before, after);
        m.set(&MCell::PdfLast(6), Some(before.clone()));
        assert_eq!(m.tex.pdf.last_link, 0);
        m.set(&MCell::PdfLast(6), Some(after));
        assert_eq!(m.tex.pdf.last_link, 17);
    }

    /// The save stack is hashed as TeX can still use it: a location
    /// saved again in one group after a `\global` assignment leaves its
    /// earlier entries dead (§282–§283), and a group's entries for
    /// distinct locations restore in any order. Two stacks that differ
    /// only so hash alike, `Rest` and the whole state, and end the group
    /// alike; a live entry that differs tells them apart.
    #[test]
    fn dead_save_stack_entries_do_not_count() {
        use crate::web::{COUNT_BASE, LEVEL_ONE};
        let (p, q) = (COUNT_BASE + 10, COUNT_BASE + 11);
        // (a group whose entries for `p` are `steps`' saves: `None` a
        // `\global` assignment, `Some(v)` a local one; then `q` locally)
        let run = |steps: &[Option<i32>], q_first: bool| {
            let mut t = engine(1);
            t.new_save_level(1).unwrap();
            if q_first {
                t.eq_word_define(q, 9).unwrap();
            }
            for (k, s) in steps.iter().enumerate() {
                match s {
                    Some(v) => t.eq_word_define(p, *v).unwrap(),
                    None => t.geq_word_define(p, 100 + i32::try_from(k).unwrap()),
                }
            }
            if !q_first {
                t.eq_word_define(q, 9).unwrap();
            }
            t
        };
        let served = |_: &[u8]| true;
        let hashes = |t: &Tex<H0, CellTracker>| (t.rest_hash_served(&served), t.state_hash());
        // (the global value saved last is 103 in both: the entries of
        // `p` below it are dead)
        let a = run(&[Some(1), None, Some(2), None, Some(3)], false);
        let b = run(&[Some(1), Some(7), Some(2), None, Some(3)], true);
        assert!(a.save_ptr > b.save_ptr, "a holds more entries");
        assert_eq!(hashes(&a), hashes(&b), "the same live entries");
        // (another global value saved: a live entry differs)
        let c = run(&[Some(1), None, Some(2), Some(5), None, Some(3)], false);
        assert_ne!(hashes(&a).0, hashes(&c).0, "a live entry differs");
        // and the group's end leaves them alike
        let end = |mut t: Tex<H0, CellTracker>| {
            t.unsave().unwrap();
            assert_eq!(t.cur_level, LEVEL_ONE);
            let at = |l: i32| t.eqtb[usize::try_from(l).unwrap()].int();
            assert_eq!((at(p), at(q)), (103, 0));
        };
        end(a);
        end(b);
    }
}
