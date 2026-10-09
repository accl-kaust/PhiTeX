//! A step's state (DESIGN 3.17.2): what TeX threads from one command to
//! the next and no name holds: the input stack and the buffer, the
//! semantic nest, the save stack, the conditionals, `align_state` and
//! the `\afterassignment` token.

use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::host::Host;
use crate::ssa::rebuild::InputState;
use crate::ssa::{Fam, SVal, Slot, Versions, set_value, slot_value};
use crate::tex::Tex;
use crate::track::{Tracker, list, save, scalar};
use partex_ssa::Version;

use super::version::slot_version;

/// The state between two commands.
#[derive(Clone)]
pub struct PState {
    pub(crate) input: InputState,
    /// The state's slots, in the order they are put back.
    pub(crate) slots: Vec<(Slot, SVal)>,
    /// The job is over: its history.
    pub(crate) finished: Option<i32>,
    /// The next step's key: where it begins, and how many steps began
    /// there before it (not in the version).
    pub(crate) kbase: u64,
    pub(crate) k: u32,
}

impl core::fmt::Debug for PState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "PState({} slots, finished {:?})",
            self.slots.len(),
            self.finished
        )
    }
}

/// The state's slots in `t` now: the nest whole, then the current
/// level's fields and the alignment's, the save stack's pointers, chain
/// and entries, the conditionals, and two scalars.
pub(crate) fn state_slots<H: Host, T: Tracker>(t: &Tex<H, T>) -> Vec<Slot> {
    let mut v = Vec::with_capacity(48);
    let d = u32::try_from(t.nest.len()).unwrap_or(0);
    v.push(Slot(Fam::List, i64::from(list::COUNT)));
    // (level 0's fields are names)
    if d > 0 {
        for f in 0..list::COUNT {
            v.push(Slot(Fam::List, i64::from(d * list::STRIDE + u32::from(f))));
        }
    }
    for k in 0..crate::track::align::COUNT {
        v.push(Slot(Fam::List, i64::from(list::COUNT + 1 + k)));
    }
    for k in [
        save::SAVE_PTR,
        save::CUR_LEVEL,
        save::CUR_GROUP,
        save::CUR_BOUNDARY,
        save::XCHAIN,
    ] {
        v.push(Slot(Fam::Save, i64::from(k)));
    }
    let sp = u32::try_from(t.save_ptr).unwrap_or(0);
    for p in 0..sp {
        v.push(Slot(Fam::Save, i64::from(save::ENTRY + p)));
    }
    let xn: u32 = t.xregs.chain_lens().iter().sum();
    for i in 0..xn {
        v.push(Slot(Fam::Save, i64::from(save::XENTRY + i)));
    }
    v.push(Slot(Fam::Cond, 0));
    v.push(Slot(Fam::Alloc, i64::from(scalar::ALIGN_STATE)));
    v.push(Slot(Fam::Alloc, i64::from(scalar::AFTER_TOKEN)));
    v
}

/// The state's version: the input's (`main`'s place aside: the core's
/// cursor is it), the nest's levels' fields, and each slot's.
fn version<H: Host, T: Tracker>(t: &Tex<H, T>, main: &[u8], slots: &[(Slot, SVal)]) -> u128 {
    let mut parts: Vec<Version> = Vec::with_capacity(slots.len() + 8);
    parts.push(Version(t.input_hash_pure(main)));
    parts.push(Version::of(&(
        t.fire_pending,
        t.page_pending,
        t.graf_stop,
        t.par_start,
        t.force_eof,
    )));
    parts.push(Version::of(&(
        t.is_in_csname,
        t.scanner_status,
        t.warning_index,
        t.expand_depth_count,
        t.no_new_control_sequence,
        t.name_in_progress,
        t.ok_to_interrupt,
        t.deletions_allowed,
        t.set_box_allowed,
    )));
    for l in t.nest.iter().skip(1) {
        for f in 0..list::COUNT {
            parts.push(Version(crate::nest::field_version(l, f)));
        }
    }
    parts.extend(slots.iter().map(|(_, v)| v.0));
    Version::node(0x7073_7461, &parts).0
}

impl PState {
    /// The state of `t` now, and its version.
    pub(crate) fn of<H: Host, T: Tracker>(
        t: &mut Tex<H, T>,
        main: &[u8],
        finished: Option<i32>,
    ) -> (PState, u128) {
        let slots: Vec<(Slot, SVal)> = state_slots(t)
            .into_iter()
            .filter_map(|s| {
                let x = slot_value(t, s)?;
                let v = slot_version(t, s).unwrap_or(Version::ABSENT.0);
                Some((s, SVal::held(Version(v), x)))
            })
            .collect();
        let ver = version(t, main, &slots);
        let input = InputState::of(t, finished.is_some());
        (
            PState {
                input,
                slots,
                finished,
                kbase: 0,
                k: 0,
            },
            ver,
        )
    }

    /// Put this state in `t`.
    pub(crate) fn set<H: Host, T: Tracker>(&self, t: &mut Tex<H, T>) {
        // (level 0 is names, the engine's: kept through the nest's change)
        let l0 = t.level_at_depth(0).cloned();
        let mut vers = Versions::default();
        for (s, v) in &self.slots {
            set_value(t, &mut vers, *s, v);
        }
        if let (Some(l0), Some(l)) = (l0, t.level_at_depth_mut(0)) {
            *l = l0;
        }
        // (a save stack that was deeper: its entries above the pointer are
        // dead, TeX never reads them)
        self.input.set(t);
    }
}

/// A shared state, for the core's values.
pub(crate) type Shared = Arc<PState>;

/// [`version`]'s parts, named (`PHITEX_PURE_SPEC=1`: a guess compared
/// with the state the run made there).
pub(crate) fn version_parts<H: Host, T: Tracker>(
    t: &Tex<H, T>,
    main: &[u8],
    slots: &[(Slot, SVal)],
) -> Vec<(alloc::string::String, u128)> {
    use alloc::format;
    let mut parts = Vec::new();
    parts.push((format!("input"), t.input_hash_pure(main)));
    parts.push((
        format!("buffer/first/last/line {}/{}/{}", t.first, t.last, t.line),
        Version::of(&(t.first, t.last, t.line)).0,
    ));
    parts.push((
        format!(
            "cur state {} index {} start {} loc {} limit {} ptr {} in_open {}",
            t.cur_input.state,
            t.cur_input.index,
            t.cur_input.start,
            t.cur_input.loc,
            t.cur_input.limit,
            t.input_ptr,
            t.in_open
        ),
        Version::of(&(
            t.cur_input.state,
            t.cur_input.index,
            t.cur_input.start,
            t.cur_input.loc,
            t.cur_input.limit,
            t.input_ptr,
            t.in_open,
        ))
        .0,
    ));
    parts.push((
        format!("flags"),
        Version::of(&(
            t.fire_pending,
            t.page_pending,
            t.graf_stop,
            t.par_start,
            t.force_eof,
        ))
        .0,
    ));
    for (d, l) in t.nest.iter().enumerate().skip(1) {
        for f in 0..list::COUNT {
            parts.push((
                format!("nest {d} field {f}"),
                crate::nest::field_version(l, f),
            ));
        }
    }
    for (s, v) in slots {
        parts.push((format!("{s:?}"), v.0.0));
    }
    parts
}

impl PState {
    /// [`PState::of`]'s version's parts (debugging).
    pub(crate) fn parts<H: Host, T: Tracker>(
        t: &mut Tex<H, T>,
        main: &[u8],
    ) -> Vec<(alloc::string::String, u128)> {
        let (ps, _) = PState::of(t, main, None);
        version_parts(t, main, &ps.slots)
    }
}
