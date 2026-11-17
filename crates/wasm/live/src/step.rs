//! What a step of the single-threaded core answers its host (`ph_step`,
//! [`crate::session`]'s `step_once`): when to step again. Apart, so its
//! cases are tested natively.

use std::time::Duration;

/// Where the document stands after a step.
#[allow(
    clippy::struct_excessive_bools,
    reason = "a step's facts, each its own, read once"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct After {
    /// A graph's save is under way (its next slice to run).
    pub saving: bool,
    /// Nothing asked, nothing pending, and a graph's save is due in this
    /// long (zero: now, the first build's of an open, or one given up
    /// for an edit before any was made); none: no save due.
    pub save_in: Option<Duration>,
    /// Only work nobody waits for is left (the build's origins).
    pub later: bool,
    /// Something was asked while the step ran.
    pub asked: bool,
    /// The step was a save's slice (not a build's step).
    pub was_save: bool,
}

/// The step's answer: 1, step again soon (the save's next slice, or a
/// save due now, or what was asked); 2, step again once the requests that
/// came are answered; 3, a save is due after the core's pause (the host
/// steps again after it, `GRAPH_IDLE`); 0, nothing to step.
///
/// A save due now is a 1, never a 3: the host steps a 3 only after its
/// pause, so the first save of an open whose build left nothing pending
/// (a loaded graph's, a change made while closed built) waited 30 s for
/// it (nightly 2026-11-16, ci-graph: "graph saved: NO after 30161 ms",
/// the save at 31.3 s).
pub(crate) fn answer(a: After) -> i32 {
    if a.saving {
        1
    } else if let Some(left) = a.save_in {
        if left.is_zero() { 1 } else { 3 }
    } else if a.later {
        2
    } else {
        // (a save made, nothing asked meanwhile: nothing more to step)
        i32::from(!a.was_save || a.asked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUIET: After = After {
        saving: false,
        save_in: None,
        later: false,
        asked: false,
        was_save: false,
    };

    /// The nightly's red: an open's first build (a loaded graph, a change
    /// made while closed built, nothing pending after it) leaves its save
    /// due at once; the step asks to be stepped again soon, not after the
    /// host's 30 s pause. Likewise after an edit that came during (or just
    /// before) the first save, its build done and nothing asked since.
    #[test]
    fn a_save_due_now_is_stepped_soon() {
        let now = After {
            save_in: Some(Duration::ZERO),
            ..QUIET
        };
        assert_eq!(answer(now), 1);
        // (the pause's save, after a save was made: after the pause)
        let later = After {
            save_in: Some(Duration::from_secs(30)),
            ..QUIET
        };
        assert_eq!(answer(later), 3);
    }

    /// A save under way is stepped again soon, a slice a step, whatever
    /// else; the build's own work after it, then nothing.
    #[test]
    fn the_other_answers() {
        assert_eq!(
            answer(After {
                saving: true,
                ..QUIET
            }),
            1
        );
        assert_eq!(
            answer(After {
                later: true,
                ..QUIET
            }),
            2
        );
        assert_eq!(answer(QUIET), 1);
        assert_eq!(
            answer(After {
                was_save: true,
                ..QUIET
            }),
            0
        );
        assert_eq!(
            answer(After {
                was_save: true,
                asked: true,
                ..QUIET
            }),
            1
        );
    }
}
